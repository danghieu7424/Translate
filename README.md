# 🚀 Translate — Thư Viện Dịch Phụ Đề Chuyên Nghiệp Tencent Hy-MT2 (1.8B GGUF)

Thư viện Rust tối ưu hóa hiệu năng cao để dịch phụ đề WebVTT (`.vtt`) và SubRip (`.srt`) bằng mô hình dịch thuật chuyên sâu **Tencent Hy-MT2 1.8B GGUF**, vận hành trên nền **Native C++ Engine** (Zero-Python, không phụ thuộc môi trường Python/pip/conda).

Được thiết kế sẵn sàng tích hợp với hệ thống giao diện dòng lệnh **`cli_gui_form`** (Ratatui TUI) theo chuẩn hiển thị trong `demo_master.rs`.

---

## 🌟 Tính Năng Chính

1. **Hỗ trợ 2 chế độ nạp đầu vào linh hoạt:**
   - **Chế độ file đơn lẻ**: Dịch trực tiếp một file phụ đề `.vtt` hoặc `.srt`.
   - **Chế độ thư mục (Batch Mode)**: Tự động quét và dịch toàn bộ danh sách phụ đề có trong thư mục chỉ định (ví dụ `input/`).

2. **Tự động tải và kiểm tra mô hình GGUF:**
   - Tự động phát hiện nếu file weights chưa tồn tại và tải từ Hugging Face Hub (`tencent/Hy-MT2-1.8B-GGUF`).
   - Hỗ trợ đúng cú pháp yêu cầu:
     ```rust
     let model = Path::new("models/Hy-MT2-1.8B-Q4_K_M.gguf");
     if !model.exists() {
         ensure_hymt2_model(model).await?;
     }
     ```

3. **Cấu hình vai vế & xưng hô siêu nhẹ qua `vocabulary` (Token-Efficient):**
   - Định dạng cấu hình trực quan, tối giản dạng `k = v` hoặc `k: v`.
   - **Cơ chế Dynamic Filtering**: Chỉ nạp vào prompt những từ vựng thực sự xuất hiện trong câu đang dịch. Giúp giảm đến **95%+ token**, triệt tiêu chi phí dư thừa và ngăn chặn dịch sai xưng hô (ví dụ: `爸 = cha`, `妈 = mẹ`).
   - Hiệu đính hậu kỳ tự động đảm bảo các thuật ngữ cốt lõi luôn được giữ vững.

4. **Tương thích 100% chuẩn tiến trình của `cli_gui_form`:**
   - Cung cấp cấu trúc `TranslationProgress` hỗ trợ chuyển đổi trực tiếp sang `cli_gui_form::TaskWidget` (`to_task_widget`, `update_task_widget`).
   - Hỗ trợ đầy đủ phong cách **Parallelogram (Cyberpunk)** và **Line (Vercel Minimalist)** chuẩn hóa như trong `demo_master.rs`.
   - Đi kèm bộ in tiến trình CLI Terminal cực đẹp khi không mở TUI toàn màn hình.

5. **Tối ưu tốc độ Native C++:**
   - Khởi chạy engine C++ độc lập qua loopback localhost, giải phóng hoàn toàn bộ nhớ RAM và đóng tiến trình khi hoàn thành (RAII Guard).

---

## 📂 Cấu Trúc Thư Mục

```text
Translate/
├── cli_gui_form/           # Thư viện TUI chuẩn
├── hymt_bin/               # Binary Native C++ engine (llama-server.exe & dlls)
├── input/                  # Thư mục phụ đề gốc (.vtt, .srt)
├── models/                 # Thư mục lưu trữ mô hình GGUF
│   └── Hy-MT2-1.8B-Q4_K_M.gguf
├── output/                 # Thư mục xuất phụ đề đã dịch (.vi.vtt, .vi.srt)
├── src/
│   ├── downloader.rs       # Tự động tải mô hình từ Hugging Face
│   ├── engine.rs           # Quản lý Native C++ Engine (Hy-MT2 1.8B)
│   ├── lib.rs              # Public API của thư viện
│   ├── progress.rs         # Định dạng tiến trình chuẩn cli_gui_form
│   ├── subtitle.rs         # Parser & Writer cho VTT & SRT
│   ├── translator.rs       # Điều phối dịch thuật toàn diện
│   └── vocabulary.rs       # Quản lý bảng từ vựng & xưng hô vai vế
├── examples/
│   └── demo_translate.rs   # Ví dụ mẫu kiểm thử thực tế
├── vocabulary.txt          # File cấu hình xưng hô & thuật ngữ
├── Cargo.toml
├── LICENSE                 # Giấy phép MIT
└── README.md
```

---

## 📝 Định Dạng File Từ Vựng `vocabulary.txt`

Sử dụng định dạng văn bản thô đơn giản, hỗ trợ chú thích bằng `#` hoặc `//`:

```ini
# ==============================================================
# BẢNG TỪ VỰNG XƯNG HÔ & THUẬT NGỮ CHUYÊN BIỆT
# Cú pháp: <từ_gốc> = <bản_dịch>
# ==============================================================

# Xưng hô gia đình
爸 = cha
妈 = mẹ
哥哥 = anh trai
弟弟 = em trai
姐姐 = chị gái
妹妹 = em gái
老婆 = vợ
老公 = chồng

# Tu tiên / Cổ trang / Donghua
师兄 = sư huynh
师弟 = sư đệ
师姐 = sư tỷ
师妹 = sư muội
前辈 = tiền bối
晚辈 = vãn bối
道友 = đạo hữu

# Thuật ngữ & Tên nhân vật
魔域 = Ma Vực
魔帝 = Ma Đế
陆清安 = Lục Thanh An
徐清莹 = Từ Thanh Oánh
修为 = tu vi
```

---

## 🛠️ Hướng Dẫn Sử Dụng Thư Viện

### 1. Thêm dependency vào `Cargo.toml`
```toml
[dependencies]
translate = { path = "." }
cli_gui_form = { path = "./cli_gui_form" }
tokio = { version = "1", features = ["full"] }
```

### 2. Viết mã dịch phụ đề
```rust
use std::path::{Path, PathBuf};
use std::sync::Arc;
use translate::{
    ensure_hymt2_model, ProgressStyle, TerminalProgressPrinter,
    TranslationOptions, TranslationProgress, Translator,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 1. Kiểm tra mô hình theo cú pháp chuẩn:
    let model = Path::new("models/Hy-MT2-1.8B-Q4_K_M.gguf");
    if !model.exists() {
        println!("Đang tải mô hình...");
        ensure_hymt2_model(model).await?;
    }

    // 2. Thiết lập hiển thị tiến trình (tương thích cli_gui_form):
    let printer = Arc::new(TerminalProgressPrinter::new());
    let printer_clone = printer.clone();
    
    let on_progress = Arc::new(move |p: TranslationProgress| {
        // In ra màn hình terminal
        printer_clone.print(&p);

        // HOẶC cập nhật vào giao diện TUI cli_gui_form:
        // let widget = p.to_task_widget(Some(ProgressStyle::Parallelogram), None);
    });

    // 3. Tùy chọn dịch thuật (hỗ trợ cả 1 file hoặc cả thư mục):
    let options = TranslationOptions {
        // Đầu vào: file đơn lẻ hoặc thư mục:
        input_path: PathBuf::from("input/第001集.zh.vtt"), // hoặc PathBuf::from("input")
        output_dir: PathBuf::from("output"),
        model_path: model.to_path_buf(),
        vocabulary_path: Some(PathBuf::from("vocabulary.txt")),
        domain_hint: "tu tiên, donghua, cổ trang".to_string(),
        hymt_config: None,
        on_progress: Some(on_progress),
    };

    let translator = Translator::new(options);
    let generated_files = translator.run().await?;

    println!("\nĐã dịch thành công {} file!", generated_files.len());
    Ok(())
}
```

---

## ⚡ Chạy Ví Dụ Kiểm Thử Tối Ưu Tốc Độ

Biên dịch và chạy example với cờ `--release` để đạt tốc độ suy luận C++ tối đa:

```bash
cargo run --release --example demo_translate
```

Tiến trình dịch phong cách Cyberpunk sẽ hiển thị trực quan:
```text
▶  [第001集.zh.vtt]  Hy-MT2 Translation Engine: ▰▰▰▰▰▰▰▰▰▰▰▰▰▰▰▰▰▰▰▰▰▰▰▰▰ 100% [69/69 câu (00:41)] - [HY-MT2] Dịch câu #69/69 ▹ [tu tiên, donghua, cổ trang hán việt]
```

---

## 📄 Bản Quyền (License)

Dự án phát hành dưới giấy phép [MIT License](LICENSE).

