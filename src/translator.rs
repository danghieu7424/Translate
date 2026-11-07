use anyhow::{anyhow, Context, Result};
use std::fs;
use std::path::PathBuf;
use std::time::Instant;

use crate::downloader::ensure_model;
use crate::engine::{HyMtConfig, HyMtWorker};
use crate::progress::{ProgressCallback, TranslationProgress};
use crate::subtitle::{parse_subtitle_file, save_subtitle_file, SubtitleFormat};
use crate::vocabulary::Vocabulary;

/// Cấu hình yêu cầu dịch thuật phụ đề
#[derive(Clone)]
pub struct TranslationOptions {
    /// Đường dẫn đầu vào (có thể là file .vtt / .srt hoặc thư mục chứa nhiều file)
    pub input_path: PathBuf,
    /// Thư mục xuất kết quả (ví dụ: "output")
    pub output_dir: PathBuf,
    /// Đường dẫn file mô hình GGUF (mặc định: "models/Hy-MT2-1.8B-Q4_K_M.gguf")
    pub model_path: PathBuf,
    /// Đường dẫn file từ vựng cấu hình vai vế / xưng hô (ví dụ: "vocabulary.txt")
    pub vocabulary_path: Option<PathBuf>,
    /// Ngữ cảnh dịch (ví dụ: "tu tiên, donghua" hoặc "hiện đại, đời sống")
    pub domain_hint: String,
    /// Cấu hình suy luận chi tiết cho Hy-MT2
    pub hymt_config: Option<HyMtConfig>,
    /// Callback cập nhật tiến trình (dùng cho TUI TaskWidget hoặc CLI)
    pub on_progress: Option<ProgressCallback>,
}

impl Default for TranslationOptions {
    fn default() -> Self {
        Self {
            input_path: PathBuf::from("input"),
            output_dir: PathBuf::from("output"),
            model_path: PathBuf::from("models/Hy-MT2-1.8B-Q4_K_M.gguf"),
            vocabulary_path: Some(PathBuf::from("vocabulary.txt")),
            domain_hint: "tu tiên, donghua, cổ trang".to_string(),
            hymt_config: None,
            on_progress: None,
        }
    }
}

/// Bộ dịch phụ đề sử dụng mô hình Tencent Hy-MT2 1.8B GGUF
pub struct Translator {
    options: TranslationOptions,
    vocabulary: Vocabulary,
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

    /// Thực hiện dịch toàn bộ các file phụ đề theo cấu hình
    pub async fn run(&self) -> Result<Vec<PathBuf>> {
        // 1. Kiểm tra mô hình theo đúng chuẩn yêu cầu
        let model = &self.options.model_path;
        if !model.exists() {
            println!(
                "{} Mô hình chưa có tại {:?}. Đang tiến hành tải tự động...",
                cli_gui_form::Icons::INFO,
                model
            );
            ensure_model(model).await?;
        }

        // 2. Thu thập file đầu vào
        let sub_files = self.collect_subtitle_files()?;
        let total_files = sub_files.len();

        fs::create_dir_all(&self.options.output_dir)?;

        // 3. Khởi tạo Native C++ Engine
        let mut config = self.options.hymt_config.clone().unwrap_or_default();
        config.model_path = model.clone();

        println!(
            "{} Đang khởi động Native Engine cho mô hình Tencent Hy-MT2 (1.8B)...",
            cli_gui_form::Icons::RUN
        );
        let worker = HyMtWorker::new(config).await?;
        println!(
            "{} Hy-MT2 Engine đã sẵn sàng! Bắt đầu dịch {} file phụ đề...",
            cli_gui_form::Icons::SUCCESS,
            total_files
        );

        let mut output_files = Vec::new();

        for (file_idx, file_path) in sub_files.iter().enumerate() {
            let file_name = file_path
                .file_name()
                .map(|f| f.to_string_lossy().to_string())
                .unwrap_or_else(|| format!("sub_{}", file_idx + 1));

            let (format, cues) = parse_subtitle_file(file_path)?;
            let total_cues = cues.len();

            let mut translated_texts = Vec::with_capacity(total_cues);
            let start_file_time = Instant::now();

            for (cue_idx, cue) in cues.iter().enumerate() {
                let current_cue_num = cue_idx + 1;
                let elapsed_sec = start_file_time.elapsed().as_secs();
                let duration_str = format!("{:02}:{:02}", elapsed_sec / 60, elapsed_sec % 60);

                let vocab_hint = self.vocabulary.format_prompt_hint(&cue.text);
                let status_msg = format!(
                    "[HY-MT2] Dịch câu #{}/{} ▹ [{}]",
                    current_cue_num, total_cues, self.options.domain_hint
                );

                // Gửi thông tin tiến trình nếu có đăng ký callback
                if let Some(ref cb) = self.options.on_progress {
                    cb(TranslationProgress {
                        task_id: "translate_task".to_string(),
                        label: "Hy-MT2 Translation Engine".to_string(),
                        current: current_cue_num,
                        total: total_cues,
                        unit: "câu".to_string(),
                        duration: duration_str.clone(),
                        status: status_msg.clone(),
                        file_index: file_idx + 1,
                        total_files,
                        file_name: file_name.clone(),
                        source_text: cue.text.clone(),
                        translated_text: String::new(),
                    });
                }

                // Dịch câu hiện tại
                let raw_trans = worker
                    .translate(
                        &cue.text,
                        &self.options.domain_hint,
                        vocab_hint.as_deref(),
                    )
                    .await
                    .with_context(|| format!("Lỗi dịch câu #{}: {:?}", current_cue_num, cue.text))?;

                // Hiệu đính hậu kỳ theo từ điển vai vế
                let final_trans = self
                    .vocabulary
                    .apply_post_corrections(&raw_trans, &cue.text);

                if let Some(ref cb) = self.options.on_progress {
                    cb(TranslationProgress {
                        task_id: "translate_task".to_string(),
                        label: "Hy-MT2 Translation Engine".to_string(),
                        current: current_cue_num,
                        total: total_cues,
                        unit: "câu".to_string(),
                        duration: duration_str,
                        status: status_msg,
                        file_index: file_idx + 1,
                        total_files,
                        file_name: file_name.clone(),
                        source_text: cue.text.clone(),
                        translated_text: final_trans.clone(),
                    });
                }

                translated_texts.push(final_trans);
            }

            // Lưu file kết quả vào thư mục output chỉ định
            let out_file_name = if file_name.ends_with(".zh.vtt") {
                file_name.replace(".zh.vtt", ".vi.vtt")
            } else if file_name.ends_with(".zh.srt") {
                file_name.replace(".zh.srt", ".vi.srt")
            } else {
                let stem = file_path.file_stem().unwrap_or_default().to_string_lossy();
                format!("{}.vi.{}", stem, format.extension())
            };

            let out_path = self.options.output_dir.join(&out_file_name);
            save_subtitle_file(&out_path, format, &cues, &translated_texts)?;
            output_files.push(out_path);
        }

        Ok(output_files)
    }
}
