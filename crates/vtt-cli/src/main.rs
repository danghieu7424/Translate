use anyhow::{Context, Result, bail};
use clap::{ArgAction, Parser, ValueEnum};
use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};
use llm_client::{ClientConfig, LlmClient};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use translator_engine::{
    EngineProgress, GlossaryEntry, Stage1Drafter, Stage2Refiner, StageConfig, TranslationResult,
    TranslationStage, TranslatorEngine, Vocabulary,
};
use vtt_core::VttDocument;

const SOURCE_FILE: &str = "source.vtt";

#[derive(Debug, Parser)]
#[command(
    name = "vtt-trans",
    version,
    about = "Translate WebVTT subtitles through a sequential two-stage pipeline"
)]
struct Args {
    #[arg(long, value_enum, default_value_t = StageSelection::All)]
    stage: StageSelection,
    #[arg(long)]
    input: Option<PathBuf>,
    #[arg(long)]
    output: Option<PathBuf>,
    #[arg(long)]
    output_dir: Option<PathBuf>,
    #[arg(long, default_value = "work")]
    work_dir: PathBuf,
    #[arg(long)]
    vocab: Option<PathBuf>,
    #[arg(long = "caption-file", action = ArgAction::Append, value_name = "PATH")]
    caption_files: Vec<PathBuf>,
    #[arg(long, default_value_t = 20)]
    stage1_batch_size: usize,
    #[arg(long, default_value_t = 35)]
    stage2_batch_size: usize,
    #[arg(long, default_value = "http://127.0.0.1:8080/v1")]
    stage1_base_url: String,
    #[arg(long, default_value = "http://127.0.0.1:8080/v1")]
    stage2_base_url: String,
    #[arg(long, default_value = "Hy-MT2-30B-A3B")]
    stage1_model: String,
    #[arg(long, default_value = "Qwen3-30B-A3B")]
    stage2_model: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum StageSelection {
    Stage1,
    Stage2,
    All,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    validate_args(&args)?;

    let style = ProgressStyle::with_template(
        "{prefix:.bold} {wide_bar:.cyan/blue} {pos}/{len} cues | {msg} | {elapsed_precise}<{eta_precise} ({per_sec} cues/s)",
    )
    .context("invalid progress bar template")?
    .progress_chars("=>-");
    let vocabulary = load_vocabulary(args.vocab.as_deref())?;
    let caption_files = collect_caption_files(&args.caption_files)?;
    let input_files = match args.input.as_deref() {
        Some(path) => collect_inputs(path)?,
        None => vec![],
    };

    if args.stage == StageSelection::Stage2 && input_files.is_empty() {
        let engine = create_engine(&args, &args.work_dir)?;
        let vocabulary = resolve_vocabulary(
            args.stage,
            &args.work_dir,
            &caption_files,
            &vocabulary,
            &engine,
        )
        .await?;
        let engine = engine.with_vocabulary(vocabulary);
        let mut progress = progress_reporter(style);
        let result = engine
            .run_stage2_with_progress(&mut progress)
            .await
            .context("Stage 2 failed")?;
        let output = args.output.as_deref().expect("validated output path");
        let source = args.work_dir.join(SOURCE_FILE);
        write_output(output, &source, &result.vtt)?;
        print_result(&result, output);
        return Ok(());
    }

    for input in input_files {
        let folder_run = args.input.as_deref().is_some_and(Path::is_dir);
        let work_dir = if folder_run {
            directory_work_dir(&args.work_dir, args.input.as_deref().unwrap(), &input)?
        } else {
            args.work_dir.clone()
        };
        let engine = create_engine(&args, &work_dir)?;
        let vocabulary = resolve_vocabulary(
            args.stage,
            &args.work_dir,
            &caption_files,
            &vocabulary,
            &engine,
        )
        .await?;
        let engine = engine.with_vocabulary(vocabulary);
        let mut progress = progress_reporter(style.clone());

        match args.stage {
            StageSelection::Stage1 => {
                let source = fs::read_to_string(&input)
                    .with_context(|| format!("failed to read input file {}", input.display()))?;
                let cue_count = VttDocument::parse(&source)?.cues().count();
                engine
                    .run_stage1_with_progress(&source, &mut progress)
                    .await
                    .with_context(|| format!("Stage 1 failed for {}", input.display()))?;
                print_summary(cue_count, cue_count, None, None);
                eprintln!("Checkpoint: {}", work_dir.join("checkpoint.json").display());
            }
            StageSelection::Stage2 => {
                let result = engine
                    .run_stage2_with_progress(&mut progress)
                    .await
                    .with_context(|| format!("Stage 2 failed in {}", work_dir.display()))?;
                let output = output_path(&args, &input)?;
                let source = work_dir.join(SOURCE_FILE);
                write_output(&output, &source, &result.vtt)?;
                print_result(&result, &output);
            }
            StageSelection::All => {
                let source = fs::read_to_string(&input)
                    .with_context(|| format!("failed to read input file {}", input.display()))?;
                let result = engine
                    .run_with_progress(&source, &mut progress)
                    .await
                    .with_context(|| format!("translation failed for {}", input.display()))?;
                let output = output_path(&args, &input)?;
                write_output(&output, &input, &result.vtt)?;
                print_result(&result, &output);
            }
        }
    }

    Ok(())
}

fn validate_args(args: &Args) -> Result<()> {
    let input_is_dir = args.input.as_deref().is_some_and(Path::is_dir);
    if args.stage1_batch_size == 0 || args.stage2_batch_size == 0 {
        bail!("batch sizes must be greater than zero");
    }
    if args.output.is_some() && args.output_dir.is_some() {
        bail!("use either --output or --output-dir, not both");
    }

    match args.stage {
        StageSelection::Stage1 if args.input.is_none() => {
            bail!("--input is required when --stage is stage1")
        }
        StageSelection::Stage2 if args.input.is_some() && !input_is_dir => {
            bail!("--input is only accepted as a directory for batch Stage 2")
        }
        StageSelection::Stage2 if input_is_dir && args.output_dir.is_none() => {
            bail!("--output-dir is required when --input is a directory")
        }
        StageSelection::Stage2 if !input_is_dir && args.output.is_none() => {
            bail!("--output is required for a single checkpoint")
        }
        StageSelection::All if args.input.is_none() => {
            bail!("--input is required when --stage is all")
        }
        StageSelection::All if input_is_dir && args.output_dir.is_none() => {
            bail!("--output-dir is required when --input is a directory")
        }
        StageSelection::All if !input_is_dir && args.output.is_none() => {
            bail!("--output is required when --input is a file")
        }
        StageSelection::Stage1 if args.output.is_some() || args.output_dir.is_some() => {
            bail!("output options are not used with --stage stage1")
        }
        _ => {}
    }
    Ok(())
}

fn create_engine(args: &Args, work_dir: &Path) -> Result<TranslatorEngine> {
    let stage1 = Stage1Drafter::new(
        create_client(&args.stage1_base_url)?,
        StageConfig::new(&args.stage1_model),
    );
    let stage2 = Stage2Refiner::new(
        create_client(&args.stage2_base_url)?,
        StageConfig::new(&args.stage2_model),
    );
    TranslatorEngine::new(work_dir, stage1, stage2)
        .and_then(|engine| engine.with_batch_sizes(args.stage1_batch_size, args.stage2_batch_size))
        .context("invalid engine configuration")
}

fn load_vocabulary(path: Option<&Path>) -> Result<Vocabulary> {
    let Some(path) = path else {
        return Ok(Vocabulary::default());
    };
    let contents = fs::read_to_string(path)
        .with_context(|| format!("failed to read vocabulary {}", path.display()))?;
    Vocabulary::parse(&contents).with_context(|| format!("invalid vocabulary {}", path.display()))
}

fn collect_inputs(input: &Path) -> Result<Vec<PathBuf>> {
    if input.is_file() {
        return Ok(vec![input.to_path_buf()]);
    }
    if !input.is_dir() {
        bail!("input path does not exist: {}", input.display());
    }
    let mut files = fs::read_dir(input)
        .with_context(|| format!("failed to list input directory {}", input.display()))?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<std::io::Result<Vec<_>>>()?
        .into_iter()
        .filter(|path| {
            path.is_file()
                && path
                    .extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("vtt"))
                && !path
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .is_some_and(|stem| stem.to_ascii_lowercase().ends_with(".vi"))
        })
        .collect::<Vec<_>>();
    files.sort();
    if files.is_empty() {
        bail!(
            "input directory contains no source .vtt files: {}",
            input.display()
        );
    }
    Ok(files)
}

fn collect_caption_files(inputs: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for input in inputs {
        if input.is_file() {
            if !is_vtt_file(input) {
                bail!(
                    "caption file must have a .vtt extension: {}",
                    input.display()
                );
            }
            files.push(input.to_path_buf());
        } else if input.is_dir() {
            collect_caption_directory(input, &mut files)?;
        } else {
            bail!("caption path does not exist: {}", input.display());
        }
    }

    files.sort();
    files.dedup();
    if !inputs.is_empty() && files.is_empty() {
        bail!("caption paths contain no .vtt files");
    }
    Ok(files)
}

fn collect_caption_directory(directory: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    let mut entries = fs::read_dir(directory)
        .with_context(|| format!("failed to list caption directory {}", directory.display()))?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<std::io::Result<Vec<_>>>()
        .with_context(|| format!("failed to read caption directory {}", directory.display()))?;
    entries.sort();

    for path in entries {
        let file_type = fs::symlink_metadata(&path)
            .with_context(|| format!("failed to inspect caption path {}", path.display()))?
            .file_type();
        if file_type.is_dir() {
            collect_caption_directory(&path, files)?;
        } else if file_type.is_file() && is_vtt_file(&path) {
            files.push(path);
        }
    }
    Ok(())
}

fn is_vtt_file(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("vtt"))
}

fn load_caption_cues(files: &[PathBuf], vocabulary: &Vocabulary) -> Result<Vec<String>> {
    let mut captions = Vec::new();
    let mut seen = HashSet::new();
    for path in files {
        let contents = fs::read_to_string(path)
            .with_context(|| format!("failed to read caption file {}", path.display()))?;
        let document = VttDocument::parse(&contents)
            .with_context(|| format!("invalid caption WebVTT {}", path.display()))?;
        for cue in document.cues() {
            let source = cue.payload().trim();
            if is_repeated_warning(source) {
                continue;
            }

            let candidate = vocabulary.remove_known_terms(source);
            let candidate = candidate.trim_matches(|character: char| {
                character.is_whitespace() || "《》〈〉-—:：，,。.!！?？、".contains(character)
            });
            if candidate.chars().any(char::is_alphanumeric) && seen.insert(candidate.to_owned()) {
                captions.push(candidate.to_owned());
            }
        }
    }
    Ok(captions)
}

fn is_repeated_warning(source: &str) -> bool {
    source.contains("危险动作请勿模仿")
        || source.contains("危险动作请勿随意模仿")
        || source.contains("危险动作不应模仿")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AutoGlossaryCache {
    version: u32,
    caption_sha256: String,
    entries: Vec<GlossaryEntry>,
}

async fn resolve_vocabulary(
    stage: StageSelection,
    work_dir: &Path,
    caption_files: &[PathBuf],
    manual_vocabulary: &Vocabulary,
    engine: &TranslatorEngine,
) -> Result<Vocabulary> {
    let cache_path = work_dir.join("auto_glossary.json");
    let captions = if caption_files.is_empty() {
        None
    } else {
        Some(load_caption_cues(caption_files, manual_vocabulary)?)
    };
    let cached = load_auto_glossary(&cache_path)?;
    let automatic = match (stage, captions.as_ref(), cached) {
        (StageSelection::Stage2, Some(captions), Some(cache)) => {
            let fingerprint = caption_fingerprint(captions)?;
            if cache.caption_sha256 != fingerprint {
                bail!(
                    "caption files do not match the auto glossary cache in {}; rerun Stage 1",
                    work_dir.display()
                );
            }
            Vocabulary::from_entries(cache.entries).context("invalid auto glossary entries")?
        }
        (StageSelection::Stage2, Some(_), None) => {
            bail!(
                "auto glossary cache is missing in {}; run Stage 1 with --caption-file first",
                work_dir.display()
            );
        }
        (StageSelection::Stage2, None, Some(cache))
        | (StageSelection::Stage1 | StageSelection::All, None, Some(cache)) => {
            Vocabulary::from_entries(cache.entries).context("invalid auto glossary entries")?
        }
        (StageSelection::Stage1 | StageSelection::All, Some(captions), cache) => {
            let fingerprint = caption_fingerprint(captions)?;
            if let Some(cache) = cache.filter(|cache| cache.caption_sha256 == fingerprint) {
                Vocabulary::from_entries(cache.entries).context("invalid auto glossary entries")?
            } else {
                let vocabulary = engine.translate_caption_terms(captions).await?;
                let cache = AutoGlossaryCache {
                    version: 1,
                    caption_sha256: fingerprint,
                    entries: vocabulary.entries(),
                };
                fs::create_dir_all(work_dir).with_context(|| {
                    format!("failed to create work directory {}", work_dir.display())
                })?;
                fs::write(&cache_path, serde_json::to_vec_pretty(&cache)?).with_context(|| {
                    format!(
                        "failed to write auto glossary cache {}",
                        cache_path.display()
                    )
                })?;
                vocabulary
            }
        }
        (_, None, None) => Vocabulary::default(),
    };
    Ok(manual_vocabulary.clone().merge(automatic))
}

fn load_auto_glossary(path: &Path) -> Result<Option<AutoGlossaryCache>> {
    if !path.exists() {
        return Ok(None);
    }
    let contents = fs::read(path)
        .with_context(|| format!("failed to read auto glossary cache {}", path.display()))?;
    let cache: AutoGlossaryCache = serde_json::from_slice(&contents)
        .with_context(|| format!("invalid auto glossary cache {}", path.display()))?;
    if cache.version != 1 {
        bail!(
            "unsupported auto glossary cache version {} in {}",
            cache.version,
            path.display()
        );
    }
    Vocabulary::from_entries(cache.entries.clone())
        .with_context(|| format!("invalid auto glossary entries in {}", path.display()))?;
    Ok(Some(cache))
}

fn caption_fingerprint(captions: &[String]) -> Result<String> {
    let serialized = serde_json::to_vec(captions)?;
    Ok(format!("{:x}", Sha256::digest(serialized)))
}

fn directory_work_dir(base: &Path, input_dir: &Path, file: &Path) -> Result<PathBuf> {
    let folder = input_dir
        .file_name()
        .context("input directory must have a name")?;
    let stem = file.file_stem().context("input file must have a name")?;
    Ok(base.join(folder).join(stem))
}

fn output_path(args: &Args, input: &Path) -> Result<PathBuf> {
    if let Some(output) = &args.output {
        return Ok(output.clone());
    }
    let name = input
        .file_stem()
        .context("input file path must have a name")?
        .to_string_lossy();
    Ok(args
        .output_dir
        .as_ref()
        .unwrap()
        .join(format!("{name}.vi.vtt")))
}

fn create_client(base_url: &str) -> Result<LlmClient> {
    LlmClient::new(ClientConfig {
        base_url: base_url.to_owned(),
        ..ClientConfig::default()
    })
    .context("failed to configure local LLM client")
}

fn progress_reporter(style: ProgressStyle) -> impl FnMut(EngineProgress) {
    let mut active: Option<(TranslationStage, ProgressBar)> = None;
    move |update| {
        if active
            .as_ref()
            .is_none_or(|(stage, _)| *stage != update.stage)
        {
            if let Some((_, bar)) = active.take() {
                bar.finish();
            }
            let bar = ProgressBar::with_draw_target(
                Some(update.total_cues as u64),
                ProgressDrawTarget::stderr(),
            );
            bar.set_style(style.clone());
            bar.set_prefix(update.stage.to_string());
            active = Some((update.stage, bar));
        }

        if let Some((_, bar)) = active.as_ref() {
            bar.set_length(update.total_cues as u64);
            bar.set_position(update.completed_cues as u64);
            bar.set_message(format!(
                "Batch {}/{}",
                update.completed_batches, update.total_batches
            ));
            if update.completed_cues >= update.total_cues {
                bar.finish_with_message(format!(
                    "Batch {}/{} complete",
                    update.completed_batches, update.total_batches
                ));
            }
        }
    }
}

fn write_output(output: &Path, source: &Path, contents: &str) -> Result<()> {
    let parent = output.parent().filter(|path| !path.as_os_str().is_empty());
    if let Some(parent) = parent {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create output directory {}", parent.display()))?;
    }

    let source_path = fs::canonicalize(source)
        .with_context(|| format!("failed to resolve {}", source.display()))?;
    let output_path = if output.exists() {
        fs::canonicalize(output)?
    } else {
        let parent = parent.unwrap_or_else(|| Path::new("."));
        fs::canonicalize(parent)?.join(output.file_name().context("output path must name a file")?)
    };
    if source_path == output_path {
        bail!("output path must not overwrite the source WebVTT file");
    }

    fs::write(output, contents)
        .with_context(|| format!("failed to write output file {}", output.display()))
}

fn print_result(result: &TranslationResult, output: &Path) {
    for warning in &result.warnings {
        eprintln!("Warning: cue {}: {}", warning.cue_id, warning.message);
    }
    print_summary(
        result.stats.total_cues,
        result.stats.translated_cues,
        Some(result.stats.stage2_changed_cues),
        Some(result.stats.fallback_cues),
    );
    println!("Output: {}", output.display());
}

fn print_summary(
    total_cues: usize,
    translated_cues: usize,
    stage2_changed_cues: Option<usize>,
    fallback_cues: Option<usize>,
) {
    println!("Translation summary");
    println!("  Cues translated: {translated_cues}/{total_cues}");
    match stage2_changed_cues {
        Some(count) => println!("  Cues changed by Stage 2: {count}"),
        None => println!("  Cues changed by Stage 2: not run"),
    }
    match fallback_cues {
        Some(count) => println!("  Cues falling back to Stage 1 draft: {count}"),
        None => println!("  Cues falling back to Stage 1 draft: not run"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage_selection_defaults_to_all_and_accepts_explicit_stages() {
        let defaults =
            Args::try_parse_from(["vtt-trans", "--input", "source.vtt", "--output", "out.vtt"])
                .unwrap();
        assert_eq!(defaults.stage, StageSelection::All);
        assert_eq!(defaults.stage1_batch_size, 20);
        assert_eq!(defaults.stage2_batch_size, 35);
        assert!(validate_args(&defaults).is_ok());

        for (stage, expected) in [
            ("stage1", StageSelection::Stage1),
            ("stage2", StageSelection::Stage2),
            ("all", StageSelection::All),
        ] {
            let args = Args::try_parse_from([
                "vtt-trans",
                "--stage",
                stage,
                "--input",
                "source.vtt",
                "--output",
                "out.vtt",
            ])
            .unwrap();
            assert_eq!(args.stage, expected);
        }
    }

    #[test]
    fn caption_files_and_batch_sizes_are_configurable_from_cli() {
        let args = Args::try_parse_from([
            "vtt-trans",
            "--stage",
            "stage1",
            "--input",
            "source.vtt",
            "--caption-file",
            "captions-one.vtt",
            "--caption-file",
            "captions-dir",
            "--stage1-batch-size",
            "12",
            "--stage2-batch-size",
            "28",
        ])
        .unwrap();

        assert_eq!(
            args.caption_files,
            vec![
                PathBuf::from("captions-one.vtt"),
                PathBuf::from("captions-dir")
            ]
        );
        assert_eq!(args.stage1_batch_size, 12);
        assert_eq!(args.stage2_batch_size, 28);
    }

    #[test]
    fn zero_batch_size_is_rejected() {
        let args = Args::try_parse_from([
            "vtt-trans",
            "--stage",
            "stage1",
            "--input",
            "source.vtt",
            "--stage1-batch-size",
            "0",
        ])
        .unwrap();
        assert!(validate_args(&args).is_err());
    }

    #[test]
    fn stage_specific_arguments_require_only_their_inputs() {
        let stage1 =
            Args::try_parse_from(["vtt-trans", "--stage", "stage1", "--input", "source.vtt"])
                .unwrap();
        assert!(validate_args(&stage1).is_ok());

        let stage2 =
            Args::try_parse_from(["vtt-trans", "--stage", "stage2", "--output", "out.vtt"])
                .unwrap();
        assert!(validate_args(&stage2).is_ok());

        let missing_input = Args::try_parse_from(["vtt-trans", "--stage", "stage1"]).unwrap();
        assert!(validate_args(&missing_input).is_err());
        let missing_output = Args::try_parse_from(["vtt-trans", "--stage", "stage2"]).unwrap();
        assert!(validate_args(&missing_output).is_err());
    }

    #[test]
    fn output_writer_refuses_to_overwrite_the_source() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.vtt");
        fs::write(&source, "WEBVTT\n").unwrap();

        assert!(write_output(&source, &source, "translated").is_err());
        assert_eq!(fs::read_to_string(source).unwrap(), "WEBVTT\n");
    }

    #[test]
    fn directory_input_selects_only_sorted_top_level_vtt_files() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("b.VTT"), "WEBVTT\n").unwrap();
        fs::write(directory.path().join("a.vtt"), "WEBVTT\n").unwrap();
        fs::write(directory.path().join("prior.vi.vtt"), "WEBVTT\n").unwrap();
        fs::write(directory.path().join("movie.mp4"), []).unwrap();
        fs::create_dir(directory.path().join("nested")).unwrap();
        fs::write(directory.path().join("nested\\ignored.vtt"), "WEBVTT\n").unwrap();

        let files = collect_inputs(directory.path()).unwrap();
        assert_eq!(
            files,
            vec![
                directory.path().join("a.vtt"),
                directory.path().join("b.VTT")
            ]
        );
        assert_eq!(
            directory_work_dir(Path::new("work"), Path::new("in"), &files[0]).unwrap(),
            PathBuf::from("work").join("in").join("a")
        );
    }

    #[test]
    fn caption_directories_are_recursive_and_known_terms_and_warnings_are_filtered() {
        let directory = tempfile::tempdir().unwrap();
        let nested = directory.path().join("nested");
        fs::create_dir(&nested).unwrap();
        fs::write(
            directory.path().join("captions.vtt"),
            concat!(
                "WEBVTT\n\n",
                "00:00:00.000 --> 00:00:01.000\n危险动作请勿模仿\n\n",
                "00:00:01.000 --> 00:00:02.000\n《陆清安-雪天妃》\n\n",
                "00:00:02.000 --> 00:00:03.000\n《雪天妃》\n\n",
                "00:00:03.000 --> 00:00:04.000\n陆清安\n\n",
            ),
        )
        .unwrap();
        fs::write(
            nested.join("more.VTT"),
            "WEBVTT\n\n00:00:00.000 --> 00:00:01.000\n幽冥之主\n\n",
        )
        .unwrap();
        fs::write(nested.join("ignored.mp4"), []).unwrap();

        let files = collect_caption_files(&[directory.path().to_path_buf()]).unwrap();
        let vocabulary = Vocabulary::parse("陆清安 = Lục Thanh An").unwrap();
        let captions = load_caption_cues(&files, &vocabulary).unwrap();

        assert_eq!(files.len(), 2);
        assert_eq!(captions, vec!["雪天妃", "幽冥之主"]);
    }

    #[tokio::test]
    async fn cached_auto_glossary_is_merged_below_manual_vocabulary() {
        let directory = tempfile::tempdir().unwrap();
        let cache = AutoGlossaryCache {
            version: 1,
            caption_sha256: "caption-hash".to_owned(),
            entries: vec![
                GlossaryEntry {
                    src: "陆清安".to_owned(),
                    target: "wrong auto name".to_owned(),
                },
                GlossaryEntry {
                    src: "雪天妃".to_owned(),
                    target: "Tuyết Thiên Phi".to_owned(),
                },
            ],
        };
        fs::write(
            directory.path().join("auto_glossary.json"),
            serde_json::to_vec(&cache).unwrap(),
        )
        .unwrap();
        let manual = Vocabulary::parse("陆清安 = Lục Thanh An").unwrap();
        let stage1 = Stage1Drafter::new(
            create_client("http://127.0.0.1:1").unwrap(),
            StageConfig::new("draft-model"),
        );
        let stage2 = Stage2Refiner::new(
            create_client("http://127.0.0.1:1").unwrap(),
            StageConfig::new("refine-model"),
        );
        let engine = TranslatorEngine::new(directory.path(), stage1, stage2).unwrap();

        let combined = resolve_vocabulary(
            StageSelection::Stage2,
            directory.path(),
            &[],
            &manual,
            &engine,
        )
        .await
        .unwrap();

        assert_eq!(
            combined.entries(),
            vec![
                GlossaryEntry {
                    src: "陆清安".to_owned(),
                    target: "Lục Thanh An".to_owned(),
                },
                GlossaryEntry {
                    src: "雪天妃".to_owned(),
                    target: "Tuyết Thiên Phi".to_owned(),
                },
            ]
        );
    }

    #[test]
    fn directory_modes_require_an_output_directory() {
        let directory = tempfile::tempdir().unwrap();
        let input = directory.path().join("subs");
        fs::create_dir(&input).unwrap();
        fs::write(input.join("one.vtt"), "WEBVTT\n").unwrap();

        let args = Args::try_parse_from([
            "vtt-trans",
            "--stage",
            "all",
            "--input",
            input.to_str().unwrap(),
            "--output-dir",
            "out",
        ])
        .unwrap();
        assert!(validate_args(&args).is_ok());

        let missing_output = Args::try_parse_from([
            "vtt-trans",
            "--stage",
            "all",
            "--input",
            input.to_str().unwrap(),
        ])
        .unwrap();
        assert!(validate_args(&missing_output).is_err());
    }
}
