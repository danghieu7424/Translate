use cli_gui_form::{Icons, ProgressStyle, TaskWidget, Theme};
use ratatui::style::Color;
use std::sync::Arc;

/// Cấu trúc thông tin tiến trình dịch thuật chuẩn hóa, sẵn sàng tích hợp với `cli_gui_form::TaskWidget`
#[derive(Debug, Clone)]
pub struct TranslationProgress {
    /// ID định danh task (dùng cho TUI FormWidget ID)
    pub task_id: String,
    /// Nhãn hiển thị tiêu đề task
    pub label: String,
    /// Chỉ số câu hiện tại (1-based)
    pub current: usize,
    /// Tổng số câu trong file
    pub total: usize,
    /// Đơn vị đo lường (mặc định: "câu")
    pub unit: String,
    /// Thời gian đã chạy định dạng chuỗi (ví dụ: "00:15" hoặc "15s")
    pub duration: String,
    /// Chuỗi trạng thái chi tiết (ví dụ: "[HY-MT2] Dịch câu #1/208 ▹ [Xianxia]")
    pub status: String,
    /// File thứ mấy trong danh sách (1-based)
    pub file_index: usize,
    /// Tổng số file cần dịch
    pub total_files: usize,
    /// Tên file hiện tại
    pub file_name: String,
    /// Câu gốc đang dịch
    pub source_text: String,
    /// Kết quả câu vừa dịch xong
    pub translated_text: String,
}

impl TranslationProgress {
    /// Chuyển đổi trạng thái tiến trình thành một `TaskWidget` của `cli_gui_form`
    /// Chuẩn hóa theo phong cách hiển thị trong demo_master.rs
    pub fn to_task_widget(
        &self,
        style: Option<ProgressStyle>,
        color: Option<Color>,
    ) -> TaskWidget {
        let task_color = color.unwrap_or(Theme::ACCENT);
        let progress_style = style.unwrap_or(ProgressStyle::Parallelogram);

        TaskWidget::new_progress(
            &self.task_id,
            &self.label,
            self.current,
            self.total,
            &self.unit,
            &self.duration,
            &self.status,
            task_color,
        )
        .with_progress_style(progress_style)
    }

    /// Cập nhật trực tiếp vào một `TaskWidget` sẵn có trong TUI runtime
    pub fn update_task_widget(&self, widget: &mut TaskWidget) {
        widget.update_progress(self.current, &self.duration);
    }
}

/// Callback function xử lý tiến trình dịch
pub type ProgressCallback = Arc<dyn Fn(TranslationProgress) + Send + Sync>;

/// In tiến trình dịch dạng Cyberpunk/Vercel trên Terminal chuẩn (khi không bật Full TUI)
pub struct TerminalProgressPrinter {
    pub style: ProgressStyle,
}

impl TerminalProgressPrinter {
    pub fn new() -> Self {
        Self {
            style: ProgressStyle::Parallelogram,
        }
    }

    pub fn print(&self, p: &TranslationProgress) {
        let ratio = if p.total == 0 {
            0.0
        } else {
            (p.current as f32 / p.total as f32).clamp(0.0, 1.0)
        };
        let percent = (ratio * 100.0).round() as u8;
        let bar_width: usize = 25;
        let filled_len = ((bar_width as f32) * ratio).round() as usize;
        let empty_len = bar_width.saturating_sub(filled_len);

        let (filled_char, empty_char) = self.style.chars();
        let bar = format!(
            "{}{}",
            filled_char.repeat(filled_len),
            empty_char.repeat(empty_len)
        );

        let file_tag = if p.total_files > 1 {
            format!("[File {}/{}: {}] ", p.file_index, p.total_files, p.file_name)
        } else {
            format!("[{}] ", p.file_name)
        };

        eprint!(
            "\r{} {} {}: {} {:>3}% [{}/{} {} ({})] - {}",
            Icons::RUN,
            file_tag,
            p.label,
            bar,
            percent,
            p.current,
            p.total,
            p.unit,
            p.duration,
            p.status
        );
    }

    pub fn finish(&self, file_name: &str, total: usize, elapsed_sec: f32) {
        eprintln!(
            "\n{} Hoàn tất dịch file {:?}: {} câu trong {:.1}s.",
            Icons::SUCCESS,
            file_name,
            total,
            elapsed_sec
        );
    }
}

impl Default for TerminalProgressPrinter {
    fn default() -> Self {
        Self::new()
    }
}
