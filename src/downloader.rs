use anyhow::{anyhow, Context, Result};
use futures_util::StreamExt;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

pub const HYMT2_HF_REPO: &str = "tencent/Hy-MT2-1.8B-GGUF";
pub const HYMT2_GGUF_FILE: &str = "Hy-MT2-1.8B-Q4_K_M.gguf";
pub const HYMT2_DOWNLOAD_URL: &str =
    "https://huggingface.co/tencent/Hy-MT2-1.8B-GGUF/resolve/main/Hy-MT2-1.8B-Q4_K_M.gguf";

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
    // 1. Kiểm tra nếu file đã tồn tại và kích thước hợp lệ (> 500MB)
    if model_path.exists() {
        if let Ok(meta) = model_path.metadata() {
            if meta.len() > 500 * 1024 * 1024 {
                return Ok(model_path.to_path_buf());
            }
        }
    }

    // 2. Tìm kiếm ở các vị trí mặc định khác nếu có
    let candidate_paths = [
        PathBuf::from("models").join(HYMT2_GGUF_FILE),
        PathBuf::from("../models").join(HYMT2_GGUF_FILE),
        PathBuf::from("models/hymt2").join(HYMT2_GGUF_FILE),
    ];

    for cand in &candidate_paths {
        if cand.exists() {
            if let Ok(meta) = cand.metadata() {
                if meta.len() > 500 * 1024 * 1024 {
                    if let Some(parent) = model_path.parent() {
                        fs::create_dir_all(parent)?;
                    }
                    println!(
                        "[Hy-MT2] Đã tìm thấy mô hình có sẵn tại {:?}, liên kết tới {:?}",
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
    println!(
        "{} Đang tải mô hình Tencent Hy-MT2 (1.8B Q4_K_M) từ Hugging Face...",
        cli_gui_form::Icons::ARROW_DOWN
    );
    println!("   URL: {}", HYMT2_DOWNLOAD_URL);
    println!("   Đích: {:?}", model_path);

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3600))
        .build()?;

    let response = client
        .get(HYMT2_DOWNLOAD_URL)
        .send()
        .await
        .context("Không thể kết nối đến Hugging Face Hub")?;

    if !response.status().is_success() {
        return Err(anyhow!(
            "Máy chủ Hugging Face phản hồi lỗi HTTP {}",
            response.status()
        ));
    }

    let total_size = response.content_length().unwrap_or(1_133_080_448);
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

    // 1. Cùng cấp với file exe đang chạy
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            candidates.push(parent.join("hymt_bin").join("llama-server.exe"));
            candidates.push(parent.join("hymt_bin").join("hymt_server.exe"));
            candidates.push(parent.join("llama-server.exe"));
            candidates.push(parent.join("hymt_server.exe"));

            if let Some(target_dir) = parent.parent() {
                if let Some(ws_root) = target_dir.parent() {
                    candidates.push(ws_root.join("hymt_bin").join("llama-server.exe"));
                    candidates.push(ws_root.join("hymt_bin").join("hymt_server.exe"));
                }
            }
        }
    }

    // 2. Thư mục hiện tại (CWD)
    candidates.push(PathBuf::from("hymt_bin/llama-server.exe"));
    candidates.push(PathBuf::from("hymt_bin/hymt_server.exe"));
    candidates.push(PathBuf::from("bin/llama-server.exe"));
    candidates.push(PathBuf::from("llama-server.exe"));

    for p in &candidates {
        if p.exists() {
            return Ok(p.clone());
        }
    }

    Err(anyhow!(
        "Chưa tìm thấy binary llama-server.exe / hymt_server.exe trong hymt_bin/."
    ))
}
