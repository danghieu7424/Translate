use anyhow::{anyhow, Context, Result};
use std::fs;
use std::path::PathBuf;
use std::time::Instant;

use crate::downloader::ensure_model;
use crate::engine::{HyMtConfig, HyMtWorker};
use crate::progress::{ProgressCallback, TranslationProgress};
use crate::refiner::{Stage1Checkpoint, Stage2Mode, Stage2Refiner};
use crate::subtitle::{parse_subtitle_file, save_subtitle_file, SubCue, SubtitleFormat};
use crate::vocabulary::Vocabulary;

/// Cấu hình yêu cầu dịch thuật phụ đề
#[derive(Clone)]
pub struct TranslationOptions {
    /// Đường dẫn đầu vào (có thể là file .vtt / .srt hoặc thư mục chứa nhiều file)
    pub input_path: PathBuf,
    /// Thư mục xuất kết quả (ví dụ: "output")
    pub output_dir: PathBuf,
    /// Đường dẫn file mô hình GGUF Tầng 1 (mặc định: "models/Hy-MT2-7B-Q4_K_M.gguf")
    pub model_path: PathBuf,
    /// Đường dẫn file từ vựng cấu hình vai vế / xưng hô (ví dụ: "vocabulary.txt")
    pub vocabulary_path: Option<PathBuf>,
    /// Ngữ cảnh dịch (ví dụ: "tu tiên, donghua" hoặc "hiện đại, đời sống")
    pub domain_hint: String,
    /// Cấu hình suy luận chi tiết cho Hy-MT2
    pub hymt_config: Option<HyMtConfig>,
    /// Callback cập nhật tiến trình (dùng cho TUI TaskWidget hoặc CLI)
    pub on_progress: Option<ProgressCallback>,
    /// Số câu dịch mỗi lượt gọi ở Tầng 1 (Batch size, mặc định: 10). Nếu đặt 1 sẽ dịch 1:1 nghiêm ngặt.
    pub batch_size: usize,
    /// Chế độ vận hành của Tầng 2 (Alibaba Qwen 3.5: Disabled, Turbo4B, Master9B)
    pub stage2_mode: Stage2Mode,
    /// Đường dẫn file mô hình Tầng 2 tùy chỉnh (nếu có)
    pub stage2_model_path: Option<PathBuf>,
}

impl Default for TranslationOptions {
    fn default() -> Self {
        Self {
            input_path: PathBuf::from("input"),
            output_dir: PathBuf::from("output"),
            model_path: PathBuf::from("models/Hy-MT2-7B-Q4_K_M.gguf"),
            vocabulary_path: Some(PathBuf::from("vocabulary.txt")),
            domain_hint: "tu tiên, donghua, cổ trang hán việt".to_string(),
            hymt_config: None,
            on_progress: None,
            batch_size: 10,
            stage2_mode: Stage2Mode::Disabled,
            stage2_model_path: None,
        }
    }
}

/// Phát hiện và bóc tách sub chú thích: Giữ nguyên chú thích và chuyển 《...》 hoặc 【...】 thành 〈...〉
pub fn process_bracket_annotations(raw_text: &str) -> (bool, String) {
    let trimmed = raw_text.trim();
    if (trimmed.starts_with('《') && trimmed.ends_with('》'))
        || (trimmed.starts_with('〈') && trimmed.ends_with('〉'))
        || (trimmed.starts_with('【') && trimmed.ends_with('】'))
        || (trimmed.starts_with('〔') && trimmed.ends_with('〕'))
    {
        let inner = trimmed
            .trim_start_matches(['《', '〈', '【', '〔'])
            .trim_end_matches(['》', '〉', '】', '〕'])
            .trim();
        (true, inner.to_string())
    } else {
        (false, trimmed.to_string())
    }
}

/// Định dạng sub chú thích theo dạng chuẩn 〈...〉, không bị mất dấu và không bị biến thành markdown bold **.
pub fn format_translated_annotation(translated: &str, is_full_annotation: bool) -> String {
    let mut clean = translated.trim().to_string();

    // Bỏ các định dạng markdown bold **...** do model tự sinh
    if clean.starts_with("**") && clean.ends_with("**") && clean.len() >= 4 {
        clean = clean[2..clean.len() - 2].trim().to_string();
    }
    clean = clean.trim_matches(['"', '\'', '`']).trim().to_string();

    if is_full_annotation {
        // Luôn bọc chắc chắn bằng dạng 〈...〉 chuẩn theo yêu cầu
        clean = clean
            .trim_start_matches(['《', '〈', '【', '〔', '*', '_', '`'])
            .trim_end_matches(['》', '〉', '】', '〕', '*', '_', '`'])
            .trim()
            .to_string();
        format!("〈{}〉", clean)
    } else {
        // Chuyển đổi mọi 《 thành 〈 và 》 thành 〉 nếu nằm bên trong câu
        clean = clean.replace('《', "〈").replace('》', "〉");
        clean = clean.replace("**", "");
        clean
    }
}

/// Chuẩn hóa đại từ xưng hô trong văn phong tu tiên, loại bỏ sót đại từ hiện đại
pub fn normalize_xianxia_pronouns(text: &str) -> String {
    let mut s = text.to_string();
    let replacements = [
        ("Tôi ", "Ta "),
        (" tôi ", " ta "),
        (" tôi.", " ta."),
        (" tôi,", " ta,"),
        (" tôi!", " ta!"),
        (" tôi?", " ta?"),
        ("Bạn ", "Ngươi "),
        (" bạn ", " ngươi "),
        (" bạn.", " ngươi."),
        (" bạn,", " ngươi,"),
        ("người vợ", "thê tử"),
        ("vợ tôi", "thê tử của ta"),
        ("vợ ta", "thê tử của ta"),
        ("người phụ nữ", "nữ nhân"),
        ("vợ chồng", "phu thê"),
    ];

    for (from, to) in replacements {
        s = s.replace(from, to);
    }
    s
}

/// Bộ dịch phụ đề sử dụng mô hình Tencent Hy-MT2 1.8B GGUF
pub struct Translator {
    options: TranslationOptions,
    vocabulary: Vocabulary,
}

/// Đại diện cho tác vụ dịch 1 file phụ đề
struct SubJob {
    file_path: PathBuf,
    file_name: String,
    format: SubtitleFormat,
    cues: Vec<SubCue>,
    stage1_cached: Option<Vec<String>>,
}

impl Translator {
    pub fn new(options: TranslationOptions) -> Self {
        let vocabulary = if let Some(ref v_path) = options.vocabulary_path {
            Vocabulary::from_file(v_path).unwrap_or_else(|_| {
                eprintln!(
                    "{} Lưu ý: Không tìm thấy file từ vựng {:?}, chạy chế độ mặc định.",
                    cli_gui_form::Icons::WARNING,
                    v_path
                );
                Vocabulary::new()
            })
        } else {
            Vocabulary::new()
        };

        Self {
            options,
            vocabulary,
        }
    }

    /// Thu thập danh sách các file phụ đề đầu vào (Hỗ trợ 2 trường hợp: 1 file đơn hoặc thư mục)
    fn collect_subtitle_files(&self) -> Result<Vec<PathBuf>> {
        let input = &self.options.input_path;
        if !input.exists() {
            return Err(anyhow!("Đường dẫn đầu vào không tồn tại: {:?}", input));
        }

        let mut files = Vec::new();

        if input.is_file() {
            if SubtitleFormat::from_path(input).is_some() {
                files.push(input.clone());
            } else {
                return Err(anyhow!(
                    "File đầu vào không phải là định dạng phụ đề hỗ trợ (.vtt hoặc .srt): {:?}",
                    input
                ));
            }
        } else if input.is_dir() {
            for entry in fs::read_dir(input)? {
                let entry = entry?;
                let path = entry.path();
                if path.is_file() && SubtitleFormat::from_path(&path).is_some() {
                    files.push(path);
                }
            }
            files.sort();
        }

        if files.is_empty() {
            return Err(anyhow!(
                "Không tìm thấy file phụ đề .vtt hoặc .srt nào tại: {:?}",
                input
            ));
        }

        Ok(files)
    }

    /// Thực hiện dịch toàn bộ các file phụ đề theo kiến trúc Pipeline 2 tầng:
    /// - Tầng 1: Dịch thô sát nghĩa bằng Tencent Hy-MT2 (1.8B/7B) + lưu Checkpoint Cache
    /// - Tầng 2: Hiệu đính ngữ cảnh bằng Alibaba Qwen 3.5 (4B Turbo hoặc 9B Master) nếu được kích hoạt
    pub async fn run(&self) -> Result<Vec<PathBuf>> {
        // 1. Kiểm tra mô hình Tầng 1
        let model = &self.options.model_path;
        if !model.exists() {
            println!(
                "{} Mô hình Tầng 1 chưa có tại {:?}. Đang tiến hành tải tự động...",
                cli_gui_form::Icons::INFO,
                model
            );
            ensure_model(model).await?;
        }

        // 2. Thu thập file đầu vào
        let sub_files = self.collect_subtitle_files()?;
        let total_files = sub_files.len();

        fs::create_dir_all(&self.options.output_dir)?;

        let is_xianxia = {
            let h = self.options.domain_hint.to_lowercase();
            h.contains("tu tiên")
                || h.contains("xianxia")
                || h.contains("cổ trang")
                || h.contains("huyền huyễn")
                || h.contains("donghua")
        };

        // 3. Đọc dữ liệu phụ đề và kiểm tra Checkpoint Cache Tầng 1
        let mut jobs = Vec::with_capacity(total_files);
        for (file_idx, file_path) in sub_files.iter().enumerate() {
            let file_name = file_path
                .file_name()
                .map(|f| f.to_string_lossy().to_string())
                .unwrap_or_else(|| format!("sub_{}", file_idx + 1));

            let (format, cues) = parse_subtitle_file(file_path)?;
            let total_cues = cues.len();

            let cache_file = self.options.output_dir.join(format!(".stage1_{}.json", file_name));
            let stage1_cached = if let Some(ckpt) = Stage1Checkpoint::load_from_file(&cache_file) {
                if ckpt.num_cues == total_cues && ckpt.stage1_translations.len() == total_cues {
                    Some(ckpt.stage1_translations)
                } else {
                    None
                }
            } else {
                None
            };

            jobs.push(SubJob {
                file_path: file_path.clone(),
                file_name,
                format,
                cues,
                stage1_cached,
            });
        }

        // 4. TIẾN TRÌNH TẦNG 1: Dịch thô bằng Tencent Hy-MT2
        let needs_stage1 = jobs.iter().any(|j| j.stage1_cached.is_none());
        let mut stage1_all_files: Vec<Vec<String>> = Vec::with_capacity(total_files);

        if needs_stage1 {
            let mut config = self.options.hymt_config.clone().unwrap_or_default();
            config.model_path = model.clone();

            let model_tag = if model.to_string_lossy().to_lowercase().contains("7b") {
                "Hy-MT2 (7B)"
            } else {
                "Hy-MT2 (1.8B)"
            };

            println!(
                "{} Đang khởi động Native Engine cho mô hình Tencent {} (TẦNG 1)...",
                cli_gui_form::Icons::RUN,
                model_tag
            );
            let worker = HyMtWorker::new(config).await?;
            println!(
                "{} {} Engine đã sẵn sàng! Bắt đầu dịch Tầng 1 cho {} file phụ đề...",
                cli_gui_form::Icons::SUCCESS,
                model_tag,
                total_files
            );

            for (file_idx, job) in jobs.iter().enumerate() {
                if let Some(ref cached) = job.stage1_cached {
                    println!(
                        "{} File [{}] đã có Checkpoint Tầng 1, phục hồi trong 1ms.",
                        cli_gui_form::Icons::SUCCESS,
                        job.file_name
                    );
                    stage1_all_files.push(cached.clone());
                    continue;
                }

                let trans = self
                    .translate_stage1_job(&worker, job, file_idx, total_files, is_xianxia)
                    .await?;

                // Lưu Checkpoint Cache Tầng 1 xuống đĩa
                let cache_file = self.options.output_dir.join(format!(".stage1_{}.json", job.file_name));
                let ckpt = Stage1Checkpoint {
                    file_name: job.file_name.clone(),
                    num_cues: job.cues.len(),
                    domain_hint: self.options.domain_hint.clone(),
                    stage1_translations: trans.clone(),
                };
                let _ = ckpt.save_to_file(&cache_file);

                stage1_all_files.push(trans);
            }

            // Tự động giải phóng 100% RAM của Hy-MT2 Engine ngay tại đây
            drop(worker);
            println!("✓  Đã hoàn thành Tầng 1 và giải phóng RAM của Hy-MT2.");
        } else {
            for job in &jobs {
                stage1_all_files.push(job.stage1_cached.clone().unwrap());
            }
            println!(
                "✓  Tất cả {} file đều đã có sẵn Checkpoint Tầng 1. Bỏ qua Tầng 1.",
                total_files
            );
        }

        // 5. TIẾN TRÌNH TẦNG 2: Hiệu đính ngữ cảnh bằng Alibaba Qwen 3.5 (nếu kích hoạt)
        let final_all_files = if self.options.stage2_mode != Stage2Mode::Disabled {
            if let Some(refiner) = Stage2Refiner::new(
                self.options.stage2_mode.clone(),
                self.options.stage2_model_path.clone(),
                self.options.domain_hint.clone(),
                self.options.on_progress.clone(),
            )? {
                let mut refined_files = Vec::with_capacity(total_files);
                for (file_idx, job) in jobs.iter().enumerate() {
                    let refined = refiner
                        .refine(
                            &job.file_name,
                            file_idx + 1,
                            total_files,
                            &job.cues,
                            &stage1_all_files[file_idx],
                        )
                        .await?;
                    refined_files.push(refined);
                }
                refined_files
            } else {
                stage1_all_files
            }
        } else {
            stage1_all_files
        };

        // 6. Xuất các file phụ đề hoàn thiện sang output/
        let mut output_files = Vec::new();
        for (file_idx, job) in jobs.iter().enumerate() {
            let out_file_name = if job.file_name.ends_with(".zh.vtt") {
                job.file_name.replace(".zh.vtt", ".vi.vtt")
            } else if job.file_name.ends_with(".zh.srt") {
                job.file_name.replace(".zh.srt", ".vi.srt")
            } else {
                let stem = job.file_path.file_stem().unwrap_or_default().to_string_lossy();
                format!("{}.vi.{}", stem, job.format.extension())
            };

            let out_path = self.options.output_dir.join(&out_file_name);
            save_subtitle_file(&out_path, job.format, &job.cues, &final_all_files[file_idx])?;
            output_files.push(out_path);
        }

        Ok(output_files)
    }

    /// Dịch thô một file phụ đề ở Tầng 1 (hỗ trợ Batch 10 câu/lô + Fallback 1:1 an toàn)
    async fn translate_stage1_job(
        &self,
        worker: &HyMtWorker,
        job: &SubJob,
        file_idx: usize,
        total_files: usize,
        is_xianxia: bool,
    ) -> Result<Vec<String>> {
        let cues = &job.cues;
        let total_cues = cues.len();
        let mut translated_texts = Vec::with_capacity(total_cues);
        let start_file_time = Instant::now();
        let batch_size = self.options.batch_size.max(1);

        for (chunk_start_idx, chunk) in cues.chunks(batch_size).enumerate() {
            let start_idx = chunk_start_idx * batch_size;
            let chunk_len = chunk.len();

            let mut chunk_items = Vec::with_capacity(chunk_len);
            for (rel_i, cue) in chunk.iter().enumerate() {
                let (is_annotation, text_to_trans) = process_bracket_annotations(&cue.text);
                let vocab_hint = self.vocabulary.format_prompt_hint(&text_to_trans);
                chunk_items.push((start_idx + rel_i, cue, is_annotation, text_to_trans, vocab_hint));
            }

            let elapsed_sec = start_file_time.elapsed().as_secs();
            let duration_str = format!("{:02}:{:02}", elapsed_sec / 60, elapsed_sec % 60);
            let first_cue_num = start_idx + 1;
            let last_cue_num = (start_idx + chunk_len).min(total_cues);
            let status_msg = if batch_size > 1 && chunk_len > 1 {
                format!(
                    "[HY-MT2 Tầng 1] Dịch lô câu #{}-#{}/{} ▹ [{}]",
                    first_cue_num, last_cue_num, total_cues, self.options.domain_hint
                )
            } else {
                format!(
                    "[HY-MT2 Tầng 1] Dịch câu #{}/{} ▹ [{}]",
                    first_cue_num, total_cues, self.options.domain_hint
                )
            };

            if let Some(ref cb) = self.options.on_progress {
                cb(TranslationProgress {
                    task_id: "stage1_task".to_string(),
                    label: "Stage 1: Hy-MT2 Engine".to_string(),
                    current: first_cue_num,
                    total: total_cues,
                    unit: "câu".to_string(),
                    duration: duration_str.clone(),
                    status: status_msg.clone(),
                    file_index: file_idx + 1,
                    total_files,
                    file_name: job.file_name.clone(),
                    source_text: chunk[0].text.clone(),
                    translated_text: String::new(),
                });
            }

            let texts_to_translate: Vec<&str> =
                chunk_items.iter().map(|item| item.3.as_str()).collect();

            let mut combined_terms = Vec::new();
            for item in &chunk_items {
                if let Some(ref h) = item.4 {
                    for part in h.split(',') {
                        let trimmed = part.trim();
                        if !trimmed.is_empty() && !combined_terms.contains(&trimmed) {
                            combined_terms.push(trimmed);
                        }
                    }
                }
            }
            let combined_vocab_hint = if combined_terms.is_empty() {
                None
            } else {
                Some(combined_terms.join(", "))
            };

            let batch_result = if batch_size > 1 && chunk_len > 1 {
                worker
                    .translate_batch(
                        &texts_to_translate,
                        &self.options.domain_hint,
                        combined_vocab_hint.as_deref(),
                    )
                    .await?
            } else {
                None
            };

            if let Some(batch_lines) = batch_result {
                for (rel_i, raw_trans) in batch_lines.into_iter().enumerate() {
                    let (abs_idx, cue, is_annotation, text_to_trans, _) = &chunk_items[rel_i];
                    let corrected = self
                        .vocabulary
                        .apply_post_corrections(&raw_trans, text_to_trans);
                    let normalized = if is_xianxia {
                        normalize_xianxia_pronouns(&corrected)
                    } else {
                        corrected
                    };
                    let final_trans = format_translated_annotation(&normalized, *is_annotation);

                    if let Some(ref cb) = self.options.on_progress {
                        let cur_num = abs_idx + 1;
                        cb(TranslationProgress {
                            task_id: "stage1_task".to_string(),
                            label: "Stage 1: Hy-MT2 Engine".to_string(),
                            current: cur_num,
                            total: total_cues,
                            unit: "câu".to_string(),
                            duration: format!(
                                "{:02}:{:02}",
                                start_file_time.elapsed().as_secs() / 60,
                                start_file_time.elapsed().as_secs() % 60
                            ),
                            status: format!(
                                "[HY-MT2 Batch] Dịch câu #{}/{} ▹ [{}]",
                                cur_num, total_cues, self.options.domain_hint
                            ),
                            file_index: file_idx + 1,
                            total_files,
                            file_name: job.file_name.clone(),
                            source_text: cue.text.clone(),
                            translated_text: final_trans.clone(),
                        });
                    }

                    translated_texts.push(final_trans);
                }
            } else {
                // Fallback an toàn 1:1
                for item in &chunk_items {
                    let (abs_idx, cue, is_annotation, text_to_trans, vocab_hint) = item;
                    let cur_num = abs_idx + 1;

                    let raw_trans = worker
                        .translate(
                            text_to_trans,
                            &self.options.domain_hint,
                            vocab_hint.as_deref(),
                        )
                        .await
                        .with_context(|| format!("Lỗi dịch câu #{}: {:?}", cur_num, cue.text))?;

                    let corrected = self
                        .vocabulary
                        .apply_post_corrections(&raw_trans, text_to_trans);
                    let normalized = if is_xianxia {
                        normalize_xianxia_pronouns(&corrected)
                    } else {
                        corrected
                    };
                    let final_trans = format_translated_annotation(&normalized, *is_annotation);

                    if let Some(ref cb) = self.options.on_progress {
                        cb(TranslationProgress {
                            task_id: "stage1_task".to_string(),
                            label: "Stage 1: Hy-MT2 Engine".to_string(),
                            current: cur_num,
                            total: total_cues,
                            unit: "câu".to_string(),
                            duration: format!(
                                "{:02}:{:02}",
                                start_file_time.elapsed().as_secs() / 60,
                                start_file_time.elapsed().as_secs() % 60
                            ),
                            status: format!(
                                "[HY-MT2 Fallback 1:1] Câu #{}/{} ▹ [{}]",
                                cur_num, total_cues, self.options.domain_hint
                            ),
                            file_index: file_idx + 1,
                            total_files,
                            file_name: job.file_name.clone(),
                            source_text: cue.text.clone(),
                            translated_text: final_trans.clone(),
                        });
                    }

                    translated_texts.push(final_trans);
                }
            }
        }

        Ok(translated_texts)
    }
}
