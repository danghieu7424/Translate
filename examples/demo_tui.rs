use cli_gui_form::{Icons, ProgressStyle, TaskWidget, Theme};
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph},
    Frame, Terminal,
};
use std::{
    io::stdout,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::sync::mpsc;
use translate::{
    ensure_hymt2_model, TranslationOptions, TranslationProgress, Translator,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1. Kiểm tra và chọn mô hình (mặc định 7B, có thể truyền --1.8b để dùng bản 1.8B)
    let args: Vec<String> = std::env::args().collect();
    let use_1_8b = args.iter().any(|a| a == "--1.8b" || a == "1.8b");
    let model_str = if use_1_8b {
        "models/Hy-MT2-1.8B-Q4_K_M.gguf"
    } else {
        "models/Hy-MT2-7B-Q4_K_M.gguf"
    };
    let model = Path::new(model_str);

    if !model.exists() {
        println!("Đang tải mô hình {:?} từ Hugging Face...", model);
        ensure_hymt2_model(model).await?;
    }

    // 2. Thiết lập Terminal cho Ratatui TUI
    enable_raw_mode()?;
    let mut stdout = stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // 3. Khởi tạo TaskWidget chuẩn theo cli_gui_form
    let mut task_widget = TaskWidget::new_progress(
        "translate_task",
        "Hy-MT2 Xianxia Translation Engine",
        0,
        69,
        "câu",
        "00:00",
        "Đang khởi tạo Native C++ Inference Engine...",
        Theme::ACCENT,
    )
    .with_progress_style(ProgressStyle::Parallelogram);

    // Kênh truyền tiến trình từ background worker sang TUI event loop
    let (tx, mut rx) = mpsc::unbounded_channel::<TranslationProgress>();

    // Cấu hình options dịch thuật
    let non_flag_args: Vec<&String> = args
        .iter()
        .skip(1)
        .filter(|a| !a.starts_with("--") && *a != "1.8b" && *a != "7b")
        .collect();

    let input_path = if let Some(path_arg) = non_flag_args.first() {
        PathBuf::from(path_arg)
    } else {
        PathBuf::from("input/第001集.zh.vtt")
    };

    let options = TranslationOptions {
        input_path,
        output_dir: PathBuf::from("output"),
        model_path: model.to_path_buf(),
        vocabulary_path: Some(PathBuf::from("vocabulary.txt")),
        domain_hint: "tu tiên, donghua, cổ trang hán việt".to_string(),
        hymt_config: None,
        on_progress: Some(Arc::new(move |p: TranslationProgress| {
            let _ = tx.send(p);
        })),
    };

    // Chạy tiến trình dịch ở background task
    tokio::spawn(async move {
        let translator = Translator::new(options);
        let _ = translator.run().await;
    });

    let mut latest_progress: Option<TranslationProgress> = None;
    let mut is_completed = false;

    // 4. Vòng lặp giao diện TUI
    loop {
        // Nhận dữ liệu tiến trình từ background task
        while let Ok(p) = rx.try_recv() {
            if p.current == p.total && p.file_index == p.total_files && !p.translated_text.is_empty() {
                is_completed = true;
            }
            task_widget.update_progress(p.current, &p.duration);
            latest_progress = Some(p);
        }

        // Vẽ giao diện TUI
        terminal.draw(|f| {
            render_tui(f, &task_widget, latest_progress.as_ref(), is_completed);
        })?;

        // Xử lý phím bấm người dùng
        if event::poll(Duration::from_millis(50))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    match key.code {
                        KeyCode::Char('q') | KeyCode::Esc => break,
                        _ => {}
                    }
                }
            }
        }
    }

    // 5. Khôi phục Terminal khi thoát
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    println!("\n{} Đã đóng giao diện TUI an toàn.", Icons::SUCCESS);
    Ok(())
}

fn render_tui(
    f: &mut Frame,
    task_widget: &TaskWidget,
    progress: Option<&TranslationProgress>,
    is_completed: bool,
) {
    let size = f.area();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Length(3), // Header Banner
            Constraint::Length(5), // TaskWidget Progress Area
            Constraint::Min(6),    // Live Translation Subtitle Box
            Constraint::Length(3), // Footer Shortcut
        ])
        .split(size);

    // 1. Header Banner
    let header = Paragraph::new(Line::from(vec![
        Span::styled(format!(" {} ", Icons::RUN), Style::default().fg(Theme::PRIMARY).add_modifier(Modifier::BOLD)),
        Span::styled("HY-MT2 1.8B GGUF", Style::default().fg(Theme::ACCENT).add_modifier(Modifier::BOLD)),
        Span::styled(" ─ Phụ Đề Tu Tiên / Tiên Hiệp (cli_gui_form TUI Engine)", Style::default().fg(Theme::SECONDARY)),
    ]))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(Theme::MUTED)),
    );
    f.render_widget(header, chunks[0]);

    // 2. TaskWidget Progress Bar
    let task_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Theme::ACCENT))
        .title(Span::styled(" TIẾN TRÌNH DỊCH THUẬT (TASK WIDGET) ", Style::default().fg(Theme::ACCENT).add_modifier(Modifier::BOLD)));
    
    let inner_task_area = task_block.inner(chunks[1]);
    f.render_widget(task_block, chunks[1]);
    cli_gui_form::FormWidget::render(task_widget, inner_task_area, f);

    // 3. Live Subtitle Preview Box
    let (source, translated, file_name, file_tag) = if let Some(p) = progress {
        (
            p.source_text.as_str(),
            p.translated_text.as_str(),
            p.file_name.as_str(),
            format!("File {}/{}", p.file_index, p.total_files),
        )
    } else {
        ("Đang kết nối tới mô hình...", "", "", "".to_string())
    };

    let preview_lines = vec![
        Line::from(vec![
            Span::styled(format!("  {} File hiện tại: ", Icons::POINTER), Style::default().fg(Theme::MUTED)),
            Span::styled(file_name, Style::default().fg(Theme::PRIMARY).add_modifier(Modifier::BOLD)),
            Span::styled(format!(" [{}]", file_tag), Style::default().fg(Theme::SECONDARY)),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Câu gốc (Trung):    ", Style::default().fg(Theme::MUTED)),
            Span::styled(source, Style::default().fg(Color::Yellow)),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Bản dịch (Việt):    ", Style::default().fg(Theme::MUTED)),
            Span::styled(translated, Style::default().fg(Theme::ACCENT).add_modifier(Modifier::BOLD)),
        ]),
    ];

    let preview_box = Paragraph::new(preview_lines).block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(Theme::MUTED))
            .title(Span::styled(" CHI TIẾT CÂU DỊCH THỜI GIAN THỰC ", Style::default().fg(Theme::PRIMARY))),
    );
    f.render_widget(preview_box, chunks[2]);

    // 4. Footer
    let footer_text = if is_completed {
        vec![
            Span::styled(format!(" {} ", Icons::SUCCESS), Style::default().fg(Theme::EMERALD).add_modifier(Modifier::BOLD)),
            Span::styled("ĐÃ HOÀN TẤT DỊCH TOÀN BỘ! ", Style::default().fg(Theme::EMERALD).add_modifier(Modifier::BOLD)),
            Span::styled("Nhấn [q] hoặc [Esc] để thoát về màn hình console.", Style::default().fg(Theme::SECONDARY)),
        ]
    } else {
        vec![
            Span::styled(format!(" {} ", Icons::RUN), Style::default().fg(Theme::PRIMARY)),
            Span::styled("Đang dịch tự động... ", Style::default().fg(Theme::SECONDARY)),
            Span::styled("Nhấn [q] hoặc [Esc] để dừng và thoát.", Style::default().fg(Theme::MUTED)),
        ]
    };

    let footer = Paragraph::new(Line::from(footer_text)).block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(Theme::MUTED)),
    );
    f.render_widget(footer, chunks[3]);
}

