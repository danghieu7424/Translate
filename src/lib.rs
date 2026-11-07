//! # Translate - Thư viện dịch phụ đề Hy-MT2 (Tencent 1.8B GGUF)
//!
//! Thư viện hỗ trợ dịch phụ đề WebVTT (`.vtt`) và SubRip (`.srt`) từ tiếng Trung/Anh sang tiếng Việt
//! bằng mô hình dịch thuật chuyên sâu **Tencent Hy-MT2 1.8B GGUF** chạy hoàn toàn trên Native C++ Engine (Zero-Python).
//!
//! ## Tính năng nổi bật
//! - **Hỗ trợ đa định dạng đầu vào**: Nhận diện tự động file đơn lẻ hoặc thư mục chứa nhiều file phụ đề (`.vtt`, `.srt`).
//! - **Tự động tải mô hình**: Tự động tải file GGUF `Hy-MT2-1.8B-Q4_K_M.gguf` từ Hugging Face Hub nếu chưa có tại đường dẫn chỉ định.
//! - **Tối ưu hóa token qua Vocabulary**: Định dạng `vocabulary.txt` cấu hình vai vế xưng hô siêu nhẹ (`爸 = cha`, `妈 = mẹ`). Hệ thống tự động lọc động chỉ nạp các từ khóa xuất hiện trong câu vào prompt, tiết kiệm 95%+ token.
//! - **Chuẩn tiến trình `cli_gui_form`**: Cung cấp cấu trúc `TranslationProgress` tương thích 100% với `cli_gui_form::TaskWidget` và Ratatui TUI.
//! - **Hiệu năng Native cực cao**: Sử dụng Native C++ runtime (`llama-server.exe`) giúp đạt tốc độ xử lý nhanh nhất trên CPU hoặc GPU.

pub mod downloader;
pub mod engine;
pub mod progress;
pub mod subtitle;
pub mod translator;
pub mod vocabulary;

pub use cli_gui_form::{Icons, ProgressStyle, Theme};
pub use downloader::ensure_model;
pub use engine::{HyMtConfig, HyMtWorker};
pub use progress::{ProgressCallback, TerminalProgressPrinter, TranslationProgress};
pub use subtitle::{
    format_timestamp, parse_subtitle_file, parse_timestamp, save_subtitle_file, SubtitleCue,
    SubtitleFormat,
};
pub use translator::{TranslationOptions, Translator};
pub use vocabulary::Vocabulary;

use anyhow::Result;
use std::path::{Path, PathBuf};

/// Hàm tiện ích chạy nhanh quá trình dịch với tùy chọn chỉ định
pub async fn translate_subtitles(options: TranslationOptions) -> Result<Vec<PathBuf>> {
    let translator = Translator::new(options);
    translator.run().await
}

/// Hàm kiểm tra và đảm bảo mô hình Hy-MT2 GGUF đã sẵn sàng
/// Theo đúng chuẩn cú pháp:
/// ```no_run
/// use std::path::Path;
/// # async fn run() {
/// let model = Path::new("models/Hy-MT2-1.8B-Q4_K_M.gguf");
/// if !model.exists() {
///     translate::ensure_hymt2_model(model).await.unwrap();
/// }
/// # }
/// ```
pub async fn ensure_hymt2_model(model_path: &Path) -> Result<PathBuf> {
    downloader::ensure_model(model_path).await
}
