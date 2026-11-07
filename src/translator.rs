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
            domain_hint: "tu tiên, donghua, cổ trang hán việt".to_string(),
            hymt_config: None,
            on_progress: None,
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

        let model_tag = if model.to_string_lossy().to_lowercase().contains("7b") {
            "Hy-MT2 (7B)"
        } else {
            "Hy-MT2 (1.8B)"
        };

        println!(
            "{} Đang khởi động Native Engine cho mô hình Tencent {}...",
            cli_gui_form::Icons::RUN,
            model_tag
        );
        let worker = HyMtWorker::new(config).await?;
        println!(
            "{} {} Engine đã sẵn sàng! Bắt đầu dịch {} file phụ đề...",
            cli_gui_form::Icons::SUCCESS,
            model_tag,
            total_files
        );

        let is_xianxia = {
            let h = self.options.domain_hint.to_lowercase();
            h.contains("tu tiên")
                || h.contains("xianxia")
                || h.contains("cổ trang")
                || h.contains("huyền huyễn")
                || h.contains("donghua")
        };

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

                // Tách chú thích nếu có
                let (is_annotation, text_to_translate) = process_bracket_annotations(&cue.text);
                let vocab_hint = self.vocabulary.format_prompt_hint(&text_to_translate);
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
                        &text_to_translate,
                        &self.options.domain_hint,
                        vocab_hint.as_deref(),
                    )
                    .await
                    .with_context(|| format!("Lỗi dịch câu #{}: {:?}", current_cue_num, cue.text))?;

                // Hiệu đính hậu kỳ theo từ điển vai vế
                let corrected = self
                    .vocabulary
                    .apply_post_corrections(&raw_trans, &text_to_translate);

                // Chuẩn hóa đại từ Tu Tiên nếu thuộc ngữ cảnh cổ trang/tu tiên
                let normalized = if is_xianxia {
                    normalize_xianxia_pronouns(&corrected)
                } else {
                    corrected
                };

                // Đảm bảo không mất sub chú thích 《》, định dạng chuẩn sang 〈...〉
                let final_trans = format_translated_annotation(&normalized, is_annotation);

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
