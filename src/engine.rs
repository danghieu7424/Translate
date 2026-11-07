use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Deserialize)]
struct ChatCompletionResponse {
    choices: Vec<ChatChoice>,
}

#[derive(Deserialize)]
struct ChatChoice {
    message: ChatMessage,
}

#[derive(Deserialize)]
struct ChatMessage {
    content: String,
}

/// Cấu hình suy luận cho mô hình Hy-MT2 GGUF
#[derive(Debug, Clone)]
pub struct HyMtConfig {
    pub model_path: PathBuf,
    pub server_bin: Option<PathBuf>,
    pub context_size: usize,
    pub threads: usize,
    pub gpu_layers: i32,
    pub temperature: f32,
    pub max_tokens: usize,
}

impl Default for HyMtConfig {
    fn default() -> Self {
        let cpu_threads = std::thread::available_parallelism()
            .map(|n| (n.get().saturating_sub(2)).max(1))
            .unwrap_or(4);

        Self {
            model_path: PathBuf::from("models/Hy-MT2-1.8B-Q4_K_M.gguf"),
            server_bin: None,
            context_size: 2048,
            threads: cpu_threads,
            gpu_layers: 0,
            temperature: 0.3,
            max_tokens: 128,
        }
    }
}

/// Quản lý tiến trình Native C++ Engine (llama-server.exe) cho mô hình Tencent Hy-MT2
pub struct HyMtWorker {
    child: Child,
    port: u16,
    client: reqwest::Client,
    pub config: HyMtConfig,
}

impl HyMtWorker {
    /// Tìm cổng TCP còn trống trên máy
    fn find_available_port(start_port: u16) -> u16 {
        for port in start_port..(start_port + 50) {
            if TcpListener::bind(("127.0.0.1", port)).is_ok() {
                return port;
            }
        }
        start_port
    }

    /// Khởi tạo và khởi động Native C++ Engine
    pub async fn new(config: HyMtConfig) -> Result<Self> {
        let server_bin = match &config.server_bin {
            Some(p) => p.clone(),
            None => crate::downloader::find_or_download_engine()?,
        };

        if !server_bin.exists() {
            return Err(anyhow!("File thực thi engine không tồn tại: {:?}", server_bin));
        }

        if !config.model_path.exists() {
            return Err(anyhow!(
                "File mô hình GGUF không tồn tại tại {:?}. Hãy chạy ensure_model trước.",
                config.model_path
            ));
        }

        let port = Self::find_available_port(18080);
        let mut cmd = Command::new(&server_bin);

        // Đặt working directory tại thư mục chứa binary để Windows nạp đúng các DLL phụ thuộc
        if let Some(bin_dir) = server_bin.parent() {
            cmd.current_dir(bin_dir);
        }

        // Chuyển đổi đường dẫn model sang đường dẫn tuyệt đối chuẩn (tránh ảnh hưởng khi đổi current_dir)
        let abs_model_path = if config.model_path.is_absolute() {
            config.model_path.clone()
        } else {
            std::env::current_dir()
                .map(|cwd| cwd.join(&config.model_path))
                .unwrap_or_else(|_| config.model_path.clone())
        };

        cmd.arg("-m")
            .arg(&abs_model_path)
            .arg("--host")
            .arg("127.0.0.1")
            .arg("--port")
            .arg(port.to_string())
            .arg("-c")
            .arg(config.context_size.to_string())
            .arg("-t")
            .arg(config.threads.to_string())
            .arg("--log-disable");

        if config.gpu_layers > 0 {
            cmd.arg("-ngl").arg(config.gpu_layers.to_string());
        }

        let child = cmd
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| format!("Không thể khởi chạy engine tại {:?}", server_bin))?;

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(60))
            .build()?;

        // Đợi server nạp weights vào RAM
        let t0 = Instant::now();
        let timeout = Duration::from_secs(45);
        let health_url = format!("http://127.0.0.1:{}/health", port);
        let mut ready = false;

        while t0.elapsed() < timeout {
            tokio::time::sleep(Duration::from_millis(300)).await;
            if let Ok(resp) = client.get(&health_url).send().await {
                if resp.status().is_success() {
                    ready = true;
                    break;
                }
            }
        }

        if !ready {
            let mut c = child;
            let _ = c.kill();
            return Err(anyhow!(
                "Quá thời gian khởi tạo Hy-MT2 Engine (Timeout 45s tại cổng {})",
                port
            ));
        }

        Ok(Self {
            child,
            port,
            client,
            config,
        })
    }

    /// Gửi yêu cầu dịch câu qua HTTP Loopback với ngữ cảnh và chỉ thị từ vựng
    pub async fn translate(
        &self,
        text: &str,
        _domain_hint: &str,
        vocab_instruction: Option<&str>,
    ) -> Result<String> {
        let prompt = match vocab_instruction {
            Some(v) if !v.is_empty() => format!("Translate to Vietnamese ({}): {}", v, text),
            _ => format!("Translate to Vietnamese: {}", text),
        };

        let body = serde_json::json!({
            "messages": [
                { "role": "user", "content": prompt }
            ],
            "temperature": self.config.temperature,
            "max_tokens": self.config.max_tokens
        });

        let url = format!("http://127.0.0.1:{}/v1/chat/completions", self.port);
        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .context("Lỗi kết nối tới Native Hy-MT2 Engine")?;

        let parsed: ChatCompletionResponse = resp
            .json()
            .await
            .context("Lỗi đọc dữ liệu JSON từ Native Hy-MT2 Engine")?;

        let raw = parsed
            .choices
            .first()
            .map(|c| c.message.content.trim().to_string())
            .unwrap_or_default();

        let clean = raw.trim_matches('"').trim_matches('\'').trim().to_string();
        Ok(clean)
    }
}

impl Drop for HyMtWorker {
    fn drop(&mut self) {
        // Tự động thu dọn tiến trình C++ khi worker ra khỏi scope (RAII Guard)
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
