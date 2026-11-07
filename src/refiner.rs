use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::downloader::ensure_model;
use crate::engine::{HyMtConfig, HyMtWorker};
use crate::progress::{ProgressCallback, TranslationProgress};
use crate::subtitle::SubCue;
use crate::translator::{format_translated_annotation, normalize_xianxia_pronouns, process_bracket_annotations};

/// Chế độ vận hành của Tầng 2 (Stage 2: Context Refiner & Proofreader)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stage2Mode {
    /// Tắt Tầng 2 (Chỉ dùng bản dịch thô từ Tầng 1 Hy-MT2)
    Disabled,
    /// Chế độ Turbo: Alibaba Qwen 3.5 - 4B (~20-30 tokens/s, tối ưu cho phim nghìn tập)
    Turbo4B,
    /// Chế độ Master: Alibaba Qwen 3.5 - 9B (~10-15 tokens/s, chất lượng văn chương đỉnh cao)
    Master9B,
    /// Sử dụng file GGUF tùy chỉnh do người dùng chỉ định
    Custom(PathBuf),
}

impl Stage2Mode {
    pub fn default_model_path(&self) -> Option<PathBuf> {
        match self {
            Stage2Mode::Disabled => None,
            Stage2Mode::Turbo4B => Some(PathBuf::from("models/Qwen3.5-4B-Q4_K_M.gguf")),
            Stage2Mode::Master9B => Some(PathBuf::from("models/Qwen3.5-9B-Q4_K_M.gguf")),
            Stage2Mode::Custom(p) => Some(p.clone()),
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Stage2Mode::Disabled => "Disabled",
            Stage2Mode::Turbo4B => "Qwen 3.5 4B (Turbo)",
            Stage2Mode::Master9B => "Qwen 3.5 9B (Master)",
            Stage2Mode::Custom(_) => "Custom Qwen GGUF",
        }
    }
}

/// Checkpoint cache lưu trữ kết quả Tầng 1 (Hy-MT2) xuống đĩa
/// Đảm bảo tính Idempotent: Nếu Tầng 2 cần chạy lại thì nạp lại trong 1ms, tránh dịch lại Tầng 1.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Stage1Checkpoint {
    pub file_name: String,
    pub num_cues: usize,
    pub domain_hint: String,
    pub stage1_translations: Vec<String>,
}

impl Stage1Checkpoint {
    pub fn load_from_file(path: &Path) -> Option<Self> {
        let content = fs::read_to_string(path).ok()?;
        serde_json::from_str(&content).ok()
    }

    pub fn save_to_file(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let content = serde_json::to_string_pretty(self)?;
        fs::write(path, content)?;
        Ok(())
    }
}

/// Bộ hiệu đính ngữ cảnh Tầng 2 bằng mô hình Alibaba Qwen 3.5 GGUF
pub struct Stage2Refiner {
    pub mode: Stage2Mode,
    pub model_path: PathBuf,
    pub batch_size: usize,
    pub domain_hint: String,
    pub on_progress: Option<ProgressCallback>,
}

impl Stage2Refiner {
    pub fn new(
        mode: Stage2Mode,
        custom_model: Option<PathBuf>,
        domain_hint: String,
        on_progress: Option<ProgressCallback>,
    ) -> Result<Option<Self>> {
        if mode == Stage2Mode::Disabled {
            return Ok(None);
        }

        let model_path = custom_model
            .or_else(|| mode.default_model_path())
            .context("Không xác định được đường dẫn mô hình cho Tầng 2")?;

        Ok(Some(Self {
            mode,
            model_path,
            batch_size: 20, // Gom 20 câu vào một cụm ngữ cảnh để LLM hiểu mạch hội thoại
            domain_hint,
            on_progress,
        }))
    }

    /// Thực thi hiệu đính Tầng 2 cho toàn bộ các câu của một file phụ đề
    pub async fn refine(
        &self,
        file_name: &str,
        file_index: usize,
        total_files: usize,
        cues: &[SubCue],
        stage1_translations: &[String],
    ) -> Result<Vec<String>> {
        let total_cues = cues.len();
        if total_cues == 0 || stage1_translations.len() != total_cues {
            return Ok(stage1_translations.to_vec());
        }

        // Đảm bảo model Qwen 3.5 tồn tại, tự động tải nếu thiếu
        let model_path = ensure_model(&self.model_path).await?;

        println!(
            "▶  Đang nạp mô hình Tầng 2 [{}] vào RAM để trau chuốt ngữ cảnh...",
            self.mode.label()
        );

        let cpu_threads = std::thread::available_parallelism()
            .map(|n| (n.get().saturating_sub(2)).max(1))
            .unwrap_or(4);

        let config = HyMtConfig {
            model_path,
            server_bin: None,
            context_size: 4096, // Đủ lớn cho 20-30 câu song ngữ
            threads: cpu_threads,
            gpu_layers: 0,
            temperature: 0.35,
            max_tokens: 1500,
        };

        // Khởi động Native Engine cho Qwen 3.5
        let worker = HyMtWorker::new(config).await?;
        println!(
            "✓  {} Engine đã sẵn sàng! Bắt đầu hiệu đính ngữ cảnh theo batch 20 câu...",
            self.mode.label()
        );

        let start_time = Instant::now();
        let mut final_translations = Vec::with_capacity(total_cues);
        let is_xianxia = self.domain_hint.to_lowercase().contains("tu tiên")
            || self.domain_hint.to_lowercase().contains("donghua")
            || self.domain_hint.to_lowercase().contains("cổ trang");

        let system_prompt = if is_xianxia {
            "You are an expert Vietnamese literary editor specializing in Tu Tiên, Donghua, and classical Xianxia cinema. \
            Your task is to refine and polish rough Vietnamese subtitle translations while preserving exact line numbers, \
            meanings, and classical Sino-Vietnamese pronouns (STRICTLY use: ta, ngươi, thê tử, phu quân, tu vi, sát lục; NEVER use: tôi, bạn, người vợ). \
            If a line has brackets 〈...〉, you MUST PRESERVE the brackets 〈...〉 exactly."
        } else {
            "You are an expert Vietnamese subtitle dialogue editor. \
            Your task is to refine and polish rough Vietnamese subtitle translations into natural, fluent Vietnamese cinema dialogue \
            while strictly preserving exact line numbers and meanings."
        };

        for (chunk_start_idx, chunk_cues) in cues.chunks(self.batch_size).enumerate() {
            let start_idx = chunk_start_idx * self.batch_size;
            let chunk_len = chunk_cues.len();
            let end_idx = (start_idx + chunk_len).min(total_cues);

            let elapsed_sec = start_time.elapsed().as_secs();
            let duration_str = format!("{:02}:{:02}", elapsed_sec / 60, elapsed_sec % 60);

            let status_msg = format!(
                "[{}] Hiệu đính ngữ cảnh câu #{}-#{}/{} ▹ [{}]",
                self.mode.label(),
                start_idx + 1,
                end_idx,
                total_cues,
                self.domain_hint
            );

            if let Some(ref cb) = self.on_progress {
                cb(TranslationProgress {
                    task_id: "stage2_task".to_string(),
                    label: format!("Stage 2: {}", self.mode.label()),
                    current: start_idx + 1,
                    total: total_cues,
                    unit: "câu".to_string(),
                    duration: duration_str.clone(),
                    status: status_msg.clone(),
                    file_index,
                    total_files,
                    file_name: file_name.to_string(),
                    source_text: chunk_cues[0].text.clone(),
                    translated_text: stage1_translations[start_idx].clone(),
                });
            }

            // Xây dựng prompt gồm bản gốc tiếng Trung và bản dịch thô Tầng 1
            let mut prompt_lines = String::new();
            for i in 0..chunk_len {
                let abs_i = start_idx + i;
                let zh = &cues[abs_i].text;
                let vi_draft = &stage1_translations[abs_i];
                prompt_lines.push_str(&format!("{}. [Gốc: {}] -> [Bản thô: {}]\n", i + 1, zh, vi_draft));
            }

            let user_prompt = format!(
                "Below are {} numbered subtitle lines. Polish and naturalize the Vietnamese dialogue.\n\
                Output EXACTLY {} numbered lines (1. ... 2. ...) with ONLY the polished Vietnamese text:\n\n{}",
                chunk_len, chunk_len, prompt_lines
            );

            let raw_res = worker
                .chat_complete(Some(system_prompt), &user_prompt, (chunk_len * 70 + 200).min(1800))
                .await;

            // Bóc tách kết quả đánh số
            let mut parsed_lines = vec![None; chunk_len];
            if let Ok(response_text) = raw_res {
                for line in response_text.lines() {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    for target_i in 1..=chunk_len {
                        let prefixes = [
                            format!("{}.", target_i),
                            format!("{}:", target_i),
                            format!("{}、", target_i),
                            format!("[{}]", target_i),
                        ];
                        for p in &prefixes {
                            if trimmed.starts_with(p) {
                                let content = trimmed[p.len()..].trim().trim_matches('"').trim_matches('\'').trim();
                                if !content.is_empty() {
                                    parsed_lines[target_i - 1] = Some(content.to_string());
                                }
                                break;
                            }
                        }
                    }
                }
            }

            // Kiểm tra số dòng: Nếu đủ đúng chunk_len câu -> áp dụng; nếu thiếu -> an toàn giữ bản dịch Tầng 1
            let is_perfect_match = parsed_lines.iter().all(|item| item.is_some());

            for (rel_i, cue) in chunk_cues.iter().enumerate() {
                let abs_i = start_idx + rel_i;
                let draft_text = &stage1_translations[abs_i];
                let (is_annotation, _) = process_bracket_annotations(&cue.text);

                let refined_text = if is_perfect_match {
                    let text = parsed_lines[rel_i].as_ref().unwrap();
                    let normalized = if is_xianxia {
                        normalize_xianxia_pronouns(text)
                    } else {
                        text.to_string()
                    };
                    format_translated_annotation(&normalized, is_annotation)
                } else {
                    // Fallback an toàn: Giữ bản dịch Tầng 1
                    draft_text.clone()
                };

                if let Some(ref cb) = self.on_progress {
                    let cur_num = abs_i + 1;
                    cb(TranslationProgress {
                        task_id: "stage2_task".to_string(),
                        label: format!("Stage 2: {}", self.mode.label()),
                        current: cur_num,
                        total: total_cues,
                        unit: "câu".to_string(),
                        duration: format!("{:02}:{:02}", start_time.elapsed().as_secs() / 60, start_time.elapsed().as_secs() % 60),
                        status: if is_perfect_match {
                            format!("[{}] Đã trau chuốt câu #{}/{}", self.mode.label(), cur_num, total_cues)
                        } else {
                            format!("[{} Fallback Tầng 1] Giữ câu #{}/{}", self.mode.label(), cur_num, total_cues)
                        },
                        file_index,
                        total_files,
                        file_name: file_name.to_string(),
                        source_text: cue.text.clone(),
                        translated_text: refined_text.clone(),
                    });
                }

                final_translations.push(refined_text);
            }
        }

        println!(
            "\n✓  Hoàn tất Tầng 2 [{}] trong {:.2}s. Đang tự động thu hồi RAM Qwen...",
            self.mode.label(),
            start_time.elapsed().as_secs_f32()
        );

        // worker sẽ tự động Drop và kill child process ở đây (giải phóng toàn bộ RAM)
        Ok(final_translations)
    }
}
