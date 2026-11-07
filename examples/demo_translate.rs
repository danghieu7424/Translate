use std::path::{Path, PathBuf};
use std::sync::Arc;
use translate::{
    ensure_hymt2_model, ProgressStyle, TerminalProgressPrinter, TranslationOptions,
    TranslationProgress, Translator,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 1. Chỉ định đường dẫn lưu và sử dụng model theo đúng cú pháp yêu cầu:
    // Mặc định kích hoạt bản 7B (chất lượng cao nhất). Dùng cờ --1.8b nếu muốn chạy bản 1.8B siêu tốc.
    let args: Vec<String> = std::env::args().collect();
    let use_1_8b = args.iter().any(|a| a == "--1.8b" || a == "1.8b");
    let model_str = if use_1_8b {
        "models/Hy-MT2-1.8B-Q4_K_M.gguf"
    } else {
        "models/Hy-MT2-7B-Q4_K_M.gguf"
    };
    let model = Path::new(model_str);

    println!(
        "=== CHƯƠNG TRÌNH DỊCH THUẬT PHỤ ĐỀ [{}] ===",
        if use_1_8b { "TENCENT HY-MT2 1.8B" } else { "TENCENT HY-MT2 7B" }
    );

    if !model.exists() {
        println!("Mô hình {:?} chưa có trong đường dẫn, bắt đầu tải tự động từ Hugging Face...", model);
        ensure_hymt2_model(model).await?;
    } else {
        println!("{} Mô hình đã có sẵn tại: {:?}", cli_gui_form::Icons::SUCCESS, model);
    }

    // 2. Cấu hình in tiến trình tương thích với chuẩn progress của cli_gui_form
    let printer = Arc::new(TerminalProgressPrinter {
        style: ProgressStyle::Parallelogram, // Phong cách Cyberpunk Parallelogram
    });

    let printer_clone = printer.clone();
    let on_progress = Arc::new(move |p: TranslationProgress| {
        // Cập nhật lên terminal
        printer_clone.print(&p);

        // Minh họa khả năng tạo / cập nhật TaskWidget cho TUI (cli_gui_form)
        // Khi cắm vào TUI thực tế:
        // let task_widget = p.to_task_widget(Some(ProgressStyle::Parallelogram), Some(cli_gui_form::Theme::ACCENT));
        // form_manager.render(...);
    });

    // 3. Thiết lập các tham số dịch thuật:
    // Hỗ trợ cả 2 trường hợp nhập liệu:
    //   - Trường hợp A: Thư mục chứa nhiều file: cargo run --release --example demo_translate -- input
    //   - Trường hợp B: Một file duy nhất: cargo run --release --example demo_translate -- input/第001集.zh.vtt
    let non_flag_args: Vec<&String> = args
        .iter()
        .skip(1)
        .filter(|a| !a.starts_with("--") && *a != "1.8b" && *a != "7b")
        .collect();

    let input_path = if let Some(path_arg) = non_flag_args.first() {
        PathBuf::from(path_arg)
    } else {
        PathBuf::from("input/第001集.zh.vtt") // Mặc định chạy 1 file để kiểm tra nhanh
    };
    let output_dir = PathBuf::from("output");
    let vocabulary_path = PathBuf::from("vocabulary.txt");

    println!("{} Đầu vào: {:?}", cli_gui_form::Icons::POINTER, input_path);
    println!("{} Đầu ra thư mục: {:?}", cli_gui_form::Icons::POINTER, output_dir);
    println!("{} File từ vựng xưng hô: {:?}", cli_gui_form::Icons::POINTER, vocabulary_path);

    let options = TranslationOptions {
        input_path,
        output_dir: output_dir.clone(),
        model_path: model.to_path_buf(),
        vocabulary_path: Some(vocabulary_path),
        domain_hint: "tu tiên, donghua, cổ trang hán việt".to_string(),
        hymt_config: None,
        on_progress: Some(on_progress),
    };

    let translator = Translator::new(options);

    let start = std::time::Instant::now();
    let results = translator.run().await?;

    println!("\n\n{} Đã hoàn thành dịch thuật!", cli_gui_form::Icons::SUCCESS);
    println!("Thời gian thực thi: {:.2}s", start.elapsed().as_secs_f32());
    println!("Các file đã xuất:");
    for out in results {
        println!("  - {:?}", out);
    }

    Ok(())
}
