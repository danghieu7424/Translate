use anyhow::{anyhow, Context, Result};
use futures_util::StreamExt;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

pub const HYMT2_HF_REPO: &str = "tencent/Hy-MT2-1.8B-GGUF";
pub const HYMT2_GGUF_FILE: &str = "Hy-MT2-1.8B-Q4_K_M.gguf";
pub const HYMT2_1_8B_DOWNLOAD_URL: &str =
    "https://huggingface.co/tencent/Hy-MT2-1.8B-GGUF/resolve/main/Hy-MT2-1.8B-Q4_K_M.gguf";

pub const HYMT2_7B_DOWNLOAD_URL: &str =
    "https://huggingface.co/tencent/Hy-MT2-7B-GGUF/resolve/main/Hy-MT2-7B-Q4_K_M.gguf";

pub const HYMT2_DOWNLOAD_URL: &str = HYMT2_1_8B_DOWNLOAD_URL;

pub const LLAMA_WIN_CPU_ZIP_URL: &str =
    "https://github.com/ggml-org/llama.cpp/releases/download/b12182/llama-b12182-bin-win-cpu-x64.zip";

/// Đảm bảo mô hình Hy-MT2 GGUF tồn tại tại đường dẫn chỉ định.
/// Nếu chưa có, tự động tải từ Hugging Face Hub.
///
/// Ví dụ sử dụng theo đúng chuẩn yêu cầu:
/// ```no_run
/// use std::path::Path;
/// # async fn run() {
/// let model = Path::new("models/Hy-MT2-1.8B-Q4_K_M.gguf");
/// if !model.exists() {
///     translate::downloader::ensure_model(model).await.unwrap();
/// }
/// # }
/// ```
pub async fn ensure_model(model_path: &Path) -> Result<PathBuf> {
    let filename = model_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(HYMT2_GGUF_FILE);
    let is_7b = filename.to_lowercase().contains("7b");
    let min_valid_size: u64 = if is_7b {
        3 * 1024 * 1024 * 1024 // 7B Q4_K_M phải > 3GB (thực tế ~4.62GB)
    } else {
        500 * 1024 * 1024 // 1.8B Q4_K_M ~1.13GB
    };

    // 1. Kiểm tra nếu file đã tồn tại và kích thước hợp lệ
    if model_path.exists() {
        if let Ok(meta) = model_path.metadata() {
            if meta.len() >= min_valid_size {
                return Ok(model_path.to_path_buf());
            } else {
                eprintln!(
                    "{} File {:?} kích thước không khớp ({:.2} MB, cần ít nhất {:.2} MB). Đang xóa để tải lại đúng bản...",
                    cli_gui_form::Icons::WARNING,
                    model_path,
                    meta.len() as f64 / (1024.0 * 1024.0),
                    min_valid_size as f64 / (1024.0 * 1024.0)
                );
                let _ = fs::remove_file(model_path);
            }
        }
    }

    // 2. Tìm kiếm ở các vị trí mặc định khác nếu có đúng tên file này
    let candidate_paths = [
        PathBuf::from("models").join(filename),
        PathBuf::from("../models").join(filename),
        PathBuf::from("models/hymt2").join(filename),
    ];

    for cand in &candidate_paths {
        if cand.canonicalize().ok() != model_path.canonicalize().ok() && cand.exists() {
            if let Ok(meta) = cand.metadata() {
                if meta.len() >= min_valid_size {
                    if let Some(parent) = model_path.parent() {
                        fs::create_dir_all(parent)?;
                    }
                    println!(
                        "[Hy-MT2] Đã tìm thấy mô hình đúng tại {:?}, sao chép tới {:?}",
                        cand, model_path
                    );
                    let _ = fs::copy(cand, model_path);
                    if model_path.exists() {
                        return Ok(model_path.to_path_buf());
                    }
                    return Ok(cand.clone());
                }
            }
        }
    }

    // 3. Tiến hành tải trực tiếp từ Hugging Face
    if let Some(parent) = model_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let temp_dest = model_path.with_extension("gguf.download");
    let is_7b = model_path.to_string_lossy().to_lowercase().contains("7b");
    let (model_label, download_url) = if is_7b {
        ("Hy-MT2 7B (Q4_K_M)", HYMT2_7B_DOWNLOAD_URL)
    } else {
        ("Hy-MT2 1.8B (Q4_K_M)", HYMT2_1_8B_DOWNLOAD_URL)
    };

    println!(
        "{} Đang tải mô hình Tencent {} từ Hugging Face...",
        cli_gui_form::Icons::ARROW_DOWN,
        model_label
    );
    println!("   URL: {}", download_url);
    println!("   Đích: {:?}", model_path);

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(7200))
        .build()?;

    let response = client
        .get(download_url)
        .send()
        .await
        .context("Không thể kết nối đến Hugging Face Hub")?;

    if !response.status().is_success() {
        return Err(anyhow!(
            "Máy chủ Hugging Face phản hồi lỗi HTTP {}",
            response.status()
        ));
    }

    let total_size = response
        .content_length()
        .unwrap_or(if is_7b { 4_960_000_000 } else { 1_133_080_448 });
    let mut downloaded: u64 = 0;
    let mut stream = response.bytes_stream();
    let mut file = File::create(&temp_dest)
        .with_context(|| format!("Không thể tạo file tạm: {:?}", temp_dest))?;

    let start_time = Instant::now();
    let mut last_print = Instant::now();

    while let Some(chunk_res) = stream.next().await {
        let chunk = chunk_res.context("Lỗi đọc luồng tải xuống")?;
        file.write_all(&chunk)?;
        downloaded += chunk.len() as u64;

        if last_print.elapsed().as_millis() > 500 {
            let pct = (downloaded as f64 / total_size as f64) * 100.0;
            let speed_mb = (downloaded as f64 / 1024.0 / 1024.0) / start_time.elapsed().as_secs_f64().max(0.1);
            print!(
                "\r   {} Đang tải: {:.1}% [{:.1} MB / {:.1} MB] - Tốc độ: {:.2} MB/s",
                cli_gui_form::Icons::RUN,
                pct,
                downloaded as f64 / 1024.0 / 1024.0,
                total_size as f64 / 1024.0 / 1024.0,
                speed_mb
            );
            let _ = std::io::stdout().flush();
            last_print = Instant::now();
        }
    }

    println!(
        "\n{} Tải hoàn tất trong {:.1}s!",
        cli_gui_form::Icons::SUCCESS,
        start_time.elapsed().as_secs_f32()
    );

    file.flush()?;
    drop(file);

    fs::rename(&temp_dest, model_path)
        .with_context(|| format!("Không thể đổi tên file {:?} sang {:?}", temp_dest, model_path))?;

    Ok(model_path.to_path_buf())
}

/// Tìm hoặc định vị binary C++ Inference Engine (llama-server.exe / hymt_server.exe)
pub fn find_or_download_engine() -> Result<PathBuf> {
    let mut candidates = Vec::new();
    let bin_names = ["llama-server.exe", "hymt_server.exe", "llama-server", "hymt_server"];

    // 1. Cùng cấp với file exe đang chạy
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            for name in &bin_names {
                candidates.push(parent.join("hymt_bin").join(name));
                candidates.push(parent.join(name));
            }

            if let Some(target_dir) = parent.parent() {
                if let Some(ws_root) = target_dir.parent() {
                    for name in &bin_names {
                        candidates.push(ws_root.join("hymt_bin").join(name));
                        candidates.push(ws_root.join("bin").join(name));
                    }
                }
            }
        }
    }

    // 2. Thư mục hiện tại (CWD)
    for name in &bin_names {
        candidates.push(PathBuf::from("hymt_bin").join(name));
        candidates.push(PathBuf::from("bin").join(name));
        candidates.push(PathBuf::from(name));
    }

    // 3. Tìm trong biến môi trường PATH
    if let Ok(path_var) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path_var) {
            for name in &bin_names {
                candidates.push(dir.join(name));
            }
        }
    }

    for p in &candidates {
        if p.exists() {
            return Ok(p.clone());
        }
    }

    Err(anyhow!(
        "Chưa tìm thấy binary llama-server / hymt_server trong hymt_bin/ hoặc PATH."
    ))
}
