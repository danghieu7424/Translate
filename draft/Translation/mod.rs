/****
 * Module: Translation
 * Bước 6 Pipeline: Dịch phụ đề tiếng Anh đã căn chỉnh (aligned.vtt) sang tiếng Việt (vi.vtt).
 *
 * Kiến trúc Hybrid 2 tầng:
 * 1. GoogleRpcEngine (Tầng 1): Dịch toàn bộ các câu siêu nhanh, miễn phí qua Google Translate RPC.
 * 2. TermCorrector (Tầng 2 - Gemini Flash): Sửa các thuật ngữ chuyên ngành (music production, engineering)
 *    bị dịch máy sai literal ("cargo build" -> "bản dựng hàng hóa", "deep house" -> "nhà sâu",...).
 *
 * Output: <video_name>.vi.vtt trong thư mục subtitles/.
 ****/

pub mod hymt_downloader;
pub mod hymt_worker;
pub mod term_corrector;

use anyhow::{Context, Result};
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

use hymt_downloader::ensure_hymt2_model;
use hymt_worker::HyMtWorker;
use serde::{Deserialize, Serialize};
use term_corrector::TermCorrector;
use crate::shared::terminal_color::TermColorExt;
use cli_gui_form::Icons;

/****
 * Struct: HyMtStage1Cache
 * Chức năng: Checkpoint lưu trữ kết quả dịch thô Tầng 1 (Hy-MT2 1.8B) xuống đĩa.
 * Đầu vào: Ngữ cảnh domain_hint, số câu num_cues, và mảng bản dịch tiếng Việt sơ bộ.
 * Ranh giới bảo vệ: Đảm bảo tính Idempotent; nếu Tầng 2 (Gemini) cạn Quota thì lần chạy sau
 * nạp lại bản dịch trong 1ms, tránh lãng phí suy luận lại mô hình 1.8B.
 ****/
#[derive(Debug, Clone, Serialize, Deserialize)]
struct HyMtStage1Cache {
    domain_hint: String,
    num_cues: usize,
    translations: Vec<String>,
}

fn load_hymt_stage1_cache(path: &Path) -> Option<HyMtStage1Cache> {
    let content = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

fn save_hymt_stage1_cache(path: &Path, cache: &HyMtStage1Cache) {
    if let Ok(content) = serde_json::to_string_pretty(cache) {
        let _ = std::fs::write(path, content);
    }
}

#[derive(Debug, Clone)]
pub struct TranslationOutput {
    pub vi_vtt: PathBuf,
    pub num_cues: usize,
}

#[derive(Debug, Clone)]
struct SubCue {
    start_ms: usize,
    end_ms: usize,
    text: String,
}

fn parse_aligned_vtt(path: &Path) -> Result<Vec<SubCue>> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Không thể đọc file aligned VTT: {:?}", path))?;

    let mut cues = Vec::new();
    let mut cur_start = 0usize;
    let mut cur_end = 0usize;
    let mut collecting = false;
    let mut text_lines = Vec::new();

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed == "WEBVTT" || trimmed.is_empty() {
            if collecting && !text_lines.is_empty() && cur_end > cur_start {
                cues.push(SubCue {
                    start_ms: cur_start,
                    end_ms: cur_end,
                    text: text_lines.join(" "),
                });
            }
            text_lines.clear();
            collecting = false;
            continue;
        }

        if trimmed.contains("-->") {
            let parts: Vec<&str> = trimmed.split("-->").collect();
            if parts.len() == 2 {
                if let (Some(s), Some(e)) = (parse_ts(parts[0]), parse_ts(parts[1])) {
                    cur_start = s;
                    cur_end = e;
                    collecting = true;
                    text_lines.clear();
                }
            }
            continue;
        }

        if collecting {
            text_lines.push(trimmed.to_string());
        }
    }

    if collecting && !text_lines.is_empty() && cur_end > cur_start {
        cues.push(SubCue {
            start_ms: cur_start,
            end_ms: cur_end,
            text: text_lines.join(" "),
        });
    }

    Ok(cues)
}

fn parse_ts(s: &str) -> Option<usize> {
    let clean = s.trim();
    let parts: Vec<&str> = clean.split(':').collect();
    if parts.len() == 3 {
        let h = parts[0].parse::<usize>().ok()?;
        let m = parts[1].parse::<usize>().ok()?;
        let sec_parts: Vec<&str> = parts[2].split('.').collect();
        let s = sec_parts[0].parse::<usize>().ok()?;
        let ms = if sec_parts.len() > 1 {
            let raw = sec_parts[1];
            let padded = format!("{:0<3}", raw);
            padded[..3].parse::<usize>().ok()?
        } else {
            0
        };
        Some(h * 3_600_000 + m * 60_000 + s * 1_000 + ms)
    } else if parts.len() == 2 {
        let m = parts[0].parse::<usize>().ok()?;
        let sec_parts: Vec<&str> = parts[1].split('.').collect();
        let s = sec_parts[0].parse::<usize>().ok()?;
        let ms = if sec_parts.len() > 1 {
            let raw = sec_parts[1];
            let padded = format!("{:0<3}", raw);
            padded[..3].parse::<usize>().ok()?
        } else {
            0
        };
        Some(m * 60_000 + s * 1_000 + ms)
    } else {
        None
    }
}

fn format_ts(ms: usize) -> String {
    let h = ms / 3_600_000;
    let m = (ms % 3_600_000) / 60_000;
    let s = (ms % 60_000) / 1_000;
    let millis = ms % 1_000;
    format!("{:02}:{:02}:{:02}.{:03}", h, m, s, millis)
}

pub fn load_env_api_keys() -> Vec<String> {
    let mut raw_strings = Vec::new();

    if let Ok(key) = std::env::var("GEMINI_API_KEY") {
        if !key.trim().is_empty() {
            raw_strings.push(key);
        }
    }

    let mut candidate_paths = vec![
        PathBuf::from(".env"),
        PathBuf::from("aidub/.env"),
        PathBuf::from("../.env"),
    ];

    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            candidate_paths.push(exe_dir.join(".env"));
            if let Some(parent) = exe_dir.parent() {
                candidate_paths.push(parent.join(".env"));
            }
        }
    }

    for path in candidate_paths {
        if let Ok(content) = std::fs::read_to_string(&path) {
            for line in content.lines() {
                let line = line.trim();
                if line.starts_with('#') || line.is_empty() {
                    continue;
                }
                if let Some((k, v)) = line.split_once('=') {
                    if k.trim() == "GEMINI_API_KEY" {
                        raw_strings.push(v.trim().to_string());
                    }
                }
            }
        }
    }

    // Tách các key theo dấu phẩy ',' và lọc trùng lặp
    let mut unique_keys = Vec::new();
    for raw in raw_strings {
        for part in raw.split(',') {
            let key = part.trim().trim_matches('"').trim_matches('\'').trim().to_string();
            if !key.is_empty() && !unique_keys.contains(&key) {
                unique_keys.push(key);
            }
        }
    }

    unique_keys
}

/****
 * translate_subtitles
 *
 * Dịch toàn bộ phụ đề WebVTT từ tiếng Anh sang tiếng Việt:
 * 1. Đọc và bóc tách các câu tiếng Anh từ aligned.vtt.
 * 2. Tầng 1: Google Translate RPC dịch hàng loạt nhanh và miễn phí.
 * 3. Tầng 2: Gemini Flash hiệu chỉnh thuật ngữ chuyên ngành (music, drill, DAWs).
 * 4. Xuất file <video_name>.vi.vtt giữ nguyên mốc thời gian căn chỉnh.
 ****/
pub async fn translate_subtitles(
    aligned_vtt: &Path,
    tmp_dir: &Path,
    model_dir: &Path,
    domain_hint: Option<&str>,
    custom_rules: Option<&str>,
    force: bool,
    defer_on_quota: bool,
) -> Result<TranslationOutput> {
    let subs_dir = tmp_dir.join("subtitles");
    std::fs::create_dir_all(&subs_dir)?;
    let hymt_cache_file = subs_dir.join(".hymt_stage1_cache.json");
    let cache_file = subs_dir.join(".term_corrector_cache.json");

    let vi_vtt_name = aligned_vtt
        .file_name()
        .map(|f| f.to_string_lossy().replace(".aligned.vtt", ".vi.vtt"))
        .unwrap_or_else(|| "output.vi.vtt".to_string());
    let vi_vtt_path = subs_dir.join(&vi_vtt_name);

    // Cache check: Nếu đã có file vi.vtt hợp lệ và không ép dịch lại thì skip
    if !force && vi_vtt_path.exists() && vi_vtt_path.metadata().map(|m| m.len()).unwrap_or(0) > 100 {
        crate::shared::tui_events::emit_log(format!(
            "   {} {} Đã có sẵn phụ đề tiếng Việt: {:?}",
            "[Translation]".name_utils().violed(),
            Icons::SUCCESS,
            vi_vtt_path
        ));
        let cues = parse_aligned_vtt(&vi_vtt_path)?;
        return Ok(TranslationOutput {
            vi_vtt: vi_vtt_path,
            num_cues: cues.len(),
        });
    }

    if force {
        let _ = std::fs::remove_file(&hymt_cache_file);
        let _ = std::fs::remove_file(&cache_file);
        crate::shared::tui_events::emit_log(format!(
            "   {} {} Ép chạy lại dịch phụ đề (Bỏ qua cache vi.vtt và checkpoint)...",
            "[Translation]".name_utils().violed(),
            Icons::RELOAD
        ));
    }

    let cues = parse_aligned_vtt(aligned_vtt)?;
    if cues.is_empty() {
        anyhow::bail!("File phụ đề aligned rỗng, không có câu nào để dịch: {:?}", aligned_vtt);
    }

    let en_sentences: Vec<&str> = cues.iter().map(|c| c.text.as_str()).collect();
    let num_cues = en_sentences.len();

    // 1. TẦNG 1: Hy-MT2 1.8B GGUF Native Worker (Offline, Domain-Tuned)
    let effective_hint = domain_hint.unwrap_or("music production, DAW");
    let mut vi_initial: Vec<String> = Vec::with_capacity(num_cues);

    // Kiểm tra checkpoint cache Tầng 1 (Phương án A: snapshot sau khi Hy-MT2 dịch xong 100%)
    if !force {
        if let Some(cache) = load_hymt_stage1_cache(&hymt_cache_file) {
            if cache.num_cues == num_cues
                && cache.translations.len() == num_cues
                && cache.domain_hint == effective_hint
            {
                crate::shared::tui_events::emit_log(format!(
                    "   {} {} [TẦNG 1] Đã phục hồi {} câu dịch Hy-MT2 từ checkpoint cache (Ngữ cảnh: '{}'). Bỏ qua suy luận lại.",
                    "[Translation/Tầng 1]".name_utils().violed(),
                    Icons::SUCCESS,
                    num_cues,
                    effective_hint
                ));
                vi_initial = cache.translations;
            } else {
                crate::shared::tui_events::emit_log(format!(
                    "   {} {} [TẦNG 1] Checkpoint cache không khớp (câu hoặc domain_hint đã đổi). Tiến hành dịch lại...",
                    "[Translation/Tầng 1]".name_utils().violed(),
                    Icons::RELOAD
                ));
            }
        }
    }

    if vi_initial.is_empty() {
        crate::shared::tui_events::emit_log(format!(
            "   {} {} [TẦNG 1] Khởi chạy Hy-MT2 (Tencent 1.8B) dịch {} câu (Ngữ cảnh: '{}')...",
            "[Translation/Tầng 1]".name_utils().violed(),
            Icons::RUN,
            num_cues,
            effective_hint
        ));

        let gguf_path = ensure_hymt2_model(model_dir)?;
        let hymt_worker = HyMtWorker::new(&gguf_path)
            .await
            .context("Không thể khởi tạo HyMtWorker cho Tầng 1 dịch thuật")?;

        let t_start_hymt = Instant::now();

        for (i, en_text) in en_sentences.iter().enumerate() {
            let elapsed_sec = t_start_hymt.elapsed().as_secs();
            // WHY: Tiến độ phong cách Cyberpunk hiển thị nhịp dịch thực tế từng câu, chuẩn zero-emoji với Icons::RUN
            crate::shared::tui_events::emit_task_progress(
                "Hy-MT2 CYBERPUNK ENGINE [1.8B INT4]",
                i + 1,
                num_cues,
                "câu",
                format!("{}s", elapsed_sec),
                format!("{}[HY-MT2] Dịch câu #{}/{} ▹ [{}]", Icons::RUN, i + 1, num_cues, effective_hint),
            );

            let translated = hymt_worker.translate(i + 1, en_text, effective_hint).await?;
            vi_initial.push(translated);
        }

        crate::shared::tui_events::emit_log(format!(
            "   {} {} [TẦNG 1] Hoàn tất dịch sơ bộ {} câu bằng Hy-MT2 trong {:.1}s.",
            "[Translation/Tầng 1]".name_utils().violed(),
            Icons::SUCCESS,
            num_cues,
            t_start_hymt.elapsed().as_secs_f32()
        ));

        // Lưu snapshot Checkpoint Tầng 1 xuống đĩa để bảo vệ tài nguyên nếu Tầng 2 bị hết Quota
        let cache_snapshot = HyMtStage1Cache {
            domain_hint: effective_hint.to_string(),
            num_cues,
            translations: vi_initial.clone(),
        };
        save_hymt_stage1_cache(&hymt_cache_file, &cache_snapshot);
    }

    // 2. TẦNG 2: Gemini Flash TermCorrector (Bắt buộc duyệt toàn bộ các batch)
    let subtitles_dir = &subs_dir;

    let api_keys = load_env_api_keys();
    let final_vi_sentences = if !api_keys.is_empty() {
        crate::shared::tui_events::emit_log(format!(
            "   {} {} [TẦNG 2] Tìm thấy {} GEMINI_API_KEY. Duyệt toàn bộ các batch hiệu đính ngữ cảnh...",
            "[Translation/Tầng 2]".name_utils().violed(),
            Icons::SEARCH,
            api_keys.len()
        ));
        let corrector = TermCorrector::new(api_keys);
        match corrector
            .correct_terms(
                &en_sentences,
                &vi_initial,
                Some(&cache_file),
                Some(effective_hint),
                custom_rules,
            )
            .await
        {
            Ok(corrected) => {
                crate::shared::tui_events::emit_log(format!(
                    "   {} {} [TẦNG 2] Đã hiệu đính thuật ngữ chuyên ngành toàn bài thành công.",
                    "[Translation/Tầng 2]".name_utils().violed(),
                    Icons::SUCCESS
                ));
                corrected
            }
            Err(e) => {
                let err_str = e.to_string();
                if defer_on_quota && err_str.contains("QUOTA_EXHAUSTED") {
                    crate::shared::tui_events::emit_log(format!(
                        "   {} {} Hết Quota Gemini trên tất cả các Key. Giữ nguyên checkpoint để chuyển/chờ video...",
                        "[Translation/Tầng 2]".name_utils().violed(),
                        Icons::HOURGLASS
                    ));
                    // Bảo vệ RALL: Không tạo file vi.vtt dở dang và không xóa cache_file
                    anyhow::bail!("QUOTA_EXHAUSTED: {}", err_str);
                }
                crate::shared::tui_events::emit_log(format!(
                    "   {} {} TermCorrector gặp lỗi: {}. Sử dụng bản dịch Hy-MT2 chuẩn gốc.",
                    "[Translation/Tầng 2]".name_utils().violed(),
                    Icons::WARNING,
                    e
                ));
                vi_initial
            }
        }
    } else {
        crate::shared::tui_events::emit_log(format!(
            "   {} {} Không tìm thấy GEMINI_API_KEY trong .env. Sử dụng trực tiếp bản dịch chất lượng cao của Hy-MT2.",
            "[Translation]".name_utils().violed(),
            Icons::INFO
        ));
        vi_initial
    };

    let video_name = tmp_dir
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .trim_start_matches(".tmp_")
        .to_string();

    let vi_vtt_path = subtitles_dir.join(format!("{}.vi.vtt", video_name));
    let mut file = File::create(&vi_vtt_path)
        .with_context(|| format!("Không thể tạo file VTT tiếng Việt: {:?}", vi_vtt_path))?;

    writeln!(file, "WEBVTT")?;
    writeln!(file)?;

    for (idx, (cue, vi_text)) in cues.iter().zip(final_vi_sentences.iter()).enumerate() {
        writeln!(file, "{}", idx + 1)?;
        writeln!(file, "{} --> {}", format_ts(cue.start_ms), format_ts(cue.end_ms))?;
        writeln!(file, "{}", vi_text.trim())?;
        writeln!(file)?;
    }

    // Lưu thêm một bản sao vi.vtt theo yêu cầu cấu hình
    let standard_vi_vtt = subtitles_dir.join("vi.vtt");
    let _ = std::fs::copy(&vi_vtt_path, &standard_vi_vtt);

    // Dọn dẹp cache checkpoint tạm sau khi đã lưu file phụ đề hoàn tất
    let _ = std::fs::remove_file(&cache_file);
    let _ = std::fs::remove_file(&hymt_cache_file);

    Ok(TranslationOutput {
        vi_vtt: vi_vtt_path,
        num_cues,
    })
}