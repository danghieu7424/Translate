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
use crate::vocabulary::Vocabulary;

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
    pub vocab: Option<Vocabulary>,
    pub on_progress: Option<ProgressCallback>,
}

impl Stage2Refiner {
    pub fn new(
        mode: Stage2Mode,
        custom_model: Option<PathBuf>,
        domain_hint: String,
        vocab: Option<Vocabulary>,
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
            batch_size: 10, // Gom 10 câu vào một cụm ngữ cảnh để tối ưu tốc độ suy luận CPU và mạch hội thoại
            domain_hint,
            vocab,
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
            temperature: 0.05, // Nhiệt độ cực thấp để loại bỏ hoàn toàn ảo giác (zero hallucination) và bám sát thuật ngữ
            max_tokens: 1500,
        };

        // Khởi động Native Engine cho Qwen 3.5
        let worker = HyMtWorker::new(config).await?;
        println!(
            "✓  {} Engine đã sẵn sàng! Bắt đầu hiệu đính ngữ cảnh theo batch 10 câu...",
            self.mode.label()
        );

        let start_time = Instant::now();
        let mut final_translations = Vec::with_capacity(total_cues);
        let is_xianxia = self.domain_hint.to_lowercase().contains("tu tiên")
            || self.domain_hint.to_lowercase().contains("donghua")
            || self.domain_hint.to_lowercase().contains("cổ trang");

        let system_prompt = if is_xianxia {
            "You are a master Vietnamese subtitle editor for Xianxia / Donghua (Tiên hiệp, Cổ trang). \
            Your duty is to polish the rough draft into natural, fluent Vietnamese cinema dialogue while strictly preserving character relations, proper names, and Xianxia terminology.\n\
            CRITICAL RULES:\n\
            1. CHARACTER RELATION ACCURACY (DO NOT CONFUSE ROLES):\n\
               - 爹 / 爹爹 / 父亲 = cha / phụ thân (NEVER 'phu quân'!). When the daughter speaks to her father, she calls him 'cha/phụ thân' and refers to herself as 'con'.\n\
               - 女儿 / 宝贝女儿 = con gái / con gái cưng (NEVER 'thê tử'!). When a father speaks to his daughter, he calls himself 'cha/ta' and calls her 'con'.\n\
               - 儿子 = con trai. 你们 = các con (khi cha nói với các con) hoặc các ngươi.\n\
               - 妻子 / 娘子 = thê tử / nương tử. 丈夫 / 夫君 = phu quân.\n\
            2. ZERO TOLERANCE FOR HALLUCINATION & PINYIN (STRICT ENFORCEMENT):\n\
               - NEVER output Pinyin or Romanized names (NEVER 'Lu Zi Xuan', 'Qing Ying', 'Minh'). Must use Hán-Việt: 'Lục Tử Huyên', 'Từ Thanh Oánh', 'Lục Miểu Miểu'.\n\
               - NEVER distort character names: 陆淼淼 / 淼淼 is 'Lục Miểu Miểu' or 'Miểu Miểu' (NEVER 'Mèo Mèo').\n\
               - NEVER modernize Xianxia concepts into modern technology or everyday colloquial terms:\n\
                 * 传音 MUST be 'truyền âm' (NEVER 'tin nhắn').\n\
                 * 识海 MUST be 'thức hải' (NEVER 'biển nhớ' or 'biển tri thức').\n\
                 * 称帝 MUST be 'xưng đế' (NEVER 'đế đô').\n\
                 * 无垢帝位 MUST be 'Vô Cấu Đế Vị' (NEVER 'Vô Thù Đế' or 'Vô Độ Thần').\n\
                 * 藏 MUST be 'ẩn nấp' / 'ẩn giấu' (NEVER 'Nấu').\n\
                 * 甜甜圈 is 'Bánh Donut'; 草见愁 is 'Thảo Kiến Sầu'; 和离 is 'hòa ly'.\n\
            3. CINEMATIC DIALOGUE:\n\
               - Make the dialogue emotional and natural. Fix stiff word-by-word phrasing.\n\
               - Use classical pronouns: ta, ngươi, con, cha, mẫu thân, huynh, đệ, tỷ, muội.\n\
            4. SUBTITLE BRACKETS:\n\
               - If a line has brackets 〈...〉 (e.g. 〈Lục Thanh An〉, 〈Hệ thống〉), you MUST PRESERVE the brackets 〈...〉 exactly.\n\
            5. FORMAT:\n\
               - Output EXACTLY the same numbered lines (1. ... 2. ...). No explanations."
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

            // Trích xuất các thuật ngữ bắt buộc xuất hiện trong batch câu hiện tại
            let batch_zh_all = chunk_cues.iter().map(|c| c.text.as_str()).collect::<Vec<_>>().join(" ");
            let vocab_note = if let Some(ref v) = self.vocab {
                if let Some(inst) = v.format_prompt_hint(&batch_zh_all) {
                    format!("MANDATORY VOCABULARY MAPPING (STRICTLY USE THESE EXACT VIETNAMESE WORDS - DO NOT INVENT OR CHANGE):\n{}\n\n", inst)
                } else {
                    String::new()
                }
            } else {
                String::new()
            };

            // Xây dựng prompt gồm bản gốc tiếng Trung và bản dịch thô Tầng 1
            let mut prompt_lines = String::new();
            for i in 0..chunk_len {
                let abs_i = start_idx + i;
                let zh = &cues[abs_i].text;
                let vi_draft = &stage1_translations[abs_i];
                prompt_lines.push_str(&format!("{}. [Gốc]: {} | [Bản thô]: {}\n", i + 1, zh, vi_draft));
            }

            let user_prompt = format!(
                "{}Below are {} numbered subtitle lines. For each line, cross-reference the original Chinese [Gốc] with the rough draft [Bản thô] to rewrite into smooth, natural Vietnamese cinema dialogue.\n\
                STRICT REQUIREMENTS:\n\
                1. Output EXACTLY {} lines (1. ... 2. ...). Each line must contain ONLY the Vietnamese dialogue.\n\
                2. NEVER output Chinese characters or Pinyin. Everything MUST be in natural Vietnamese.\n\
                3. Do NOT include tags like 'Gốc' or 'Bản thô' or arrows like '->'.\n\n{}",
                vocab_note, chunk_len, chunk_len, prompt_lines
            );

            let raw_res = worker
                .chat_complete(Some(system_prompt), &user_prompt, (chunk_len * 70 + 200).min(1800))
                .await;

            // Bóc tách kết quả đánh số
            let mut parsed_lines = vec![None; chunk_len];
            if let Ok(response_text) = raw_res {
                let clean_text = if let Some(idx) = response_text.find("</think>") {
                    &response_text[idx + 8..]
                } else {
                    &response_text
                };

                for line in clean_text.lines() {
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

            for (rel_i, cue) in chunk_cues.iter().enumerate() {
                let abs_i = start_idx + rel_i;
                let draft_text = &stage1_translations[abs_i];
                let (is_annotation, _) = process_bracket_annotations(&cue.text);

                let refined_text = if let Some(text) = &parsed_lines[rel_i] {
                    // Loại bỏ các tiền tố [Gốc: ...] -> [Bản thô: ...] nếu mô hình trích dẫn lại
                    let mut cleaned_item = text.trim();
                    if let Some(pos) = cleaned_item.rfind("->") {
                        cleaned_item = cleaned_item[pos + 2..].trim();
                    }
                    if let Some(pos) = cleaned_item.find("Bản dịch:") {
                        cleaned_item = cleaned_item[pos + 9..].trim();
                    } else if let Some(pos) = cleaned_item.find("Bản thô:") {
                        cleaned_item = cleaned_item[pos + 8..].trim();
                    }
                    let cleaned_str = cleaned_item.trim_matches('[').trim_matches(']').trim();

                    // Kiểm tra xem dòng có vô tình chứa ký tự tiếng Trung không
                    let has_chinese = cleaned_str.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c));

                    let valid_text = if !cleaned_str.is_empty() && !has_chinese {
                        cleaned_str
                    } else {
                        draft_text.as_str()
                    };

                    let normalized = if is_xianxia {
                        normalize_xianxia_pronouns(valid_text)
                    } else {
                        valid_text.to_string()
                    };

                    let post_corrected = if let Some(ref v) = self.vocab {
                        v.apply_post_corrections(&normalized, &cue.text)
                    } else {
                        normalized
                    };

                    format_translated_annotation(&post_corrected, is_annotation)
                } else {
                    // Fallback từng câu đơn lẻ: Giữ bản dịch Tầng 1
                    draft_text.clone()
                };

                if let Some(ref cb) = self.on_progress {
                    let cur_num = abs_i + 1;
                    let is_refined = parsed_lines[rel_i].is_some();
                    cb(TranslationProgress {
                        task_id: "stage2_task".to_string(),
                        label: format!("Stage 2: {}", self.mode.label()),
                        current: cur_num,
                        total: total_cues,
                        unit: "câu".to_string(),
                        duration: format!("{:02}:{:02}", start_time.elapsed().as_secs() / 60, start_time.elapsed().as_secs() % 60),
                        status: if is_refined {
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
