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
        domain_hint: &str,
        vocab_instruction: Option<&str>,
    ) -> Result<String> {
        let hint_lower = domain_hint.to_lowercase();
        let is_xianxia = hint_lower.contains("tu tiên")
            || hint_lower.contains("xianxia")
            || hint_lower.contains("xuanhuan")
            || hint_lower.contains("huyền huyễn")
            || hint_lower.contains("donghua")
            || hint_lower.contains("manhua")
            || hint_lower.contains("cổ trang")
            || hint_lower.contains("kiếm hiệp");

        let is_modern = hint_lower.contains("hiện đại")
            || hint_lower.contains("modern")
            || hint_lower.contains("đô thị")
            || hint_lower.contains("urban");

        let style_directive = if is_xianxia {
            "Translate the Chinese text to Vietnamese in classical Tu Tiên/Xianxia novel style (STRICTLY use Sino-Vietnamese pronouns: 我=ta, 你=ngươi, 妻子=thê tử, 丈夫=phu quân, 女人=nữ nhân, 杀戮=sát lục, 修为=tu vi; NEVER use modern pronouns: tôi, bạn, người vợ, phụ nữ). Output ONLY the Vietnamese translation"
        } else if is_modern {
            "Translate to Vietnamese in modern daily life/urban register (STRICTLY use modern natural pronouns: anh/em, tôi/bạn, cậu/tớ; NEVER use archaic pronouns: tại hạ, bổn tọa, phu quân, thê tử). Output ONLY the Vietnamese translation"
        } else {
            "Translate to Vietnamese. Output ONLY the Vietnamese translation"
        };

        let prompt = match vocab_instruction {
            Some(v) if !v.is_empty() => format!("{} ({}): {}", style_directive, v, text),
            _ => format!("{}: {}", style_directive, text),
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

    /// Dịch hàng loạt một lô các câu (Batch Translation) để tối ưu tốc độ gấp nhiều lần
    /// Trả về Some(Vec<String>) nếu số câu phân tích được khớp chính xác 100% số lượng câu yêu cầu.
    /// Trả về None nếu mô hình gộp hoặc thiếu dòng (để hệ thống tự động kích hoạt fallback 1:1 an toàn).
    pub async fn translate_batch(
        &self,
        texts: &[&str],
        domain_hint: &str,
        vocab_instruction: Option<&str>,
    ) -> Result<Option<Vec<String>>> {
        let n = texts.len();
        if n == 0 {
            return Ok(Some(Vec::new()));
        }
        if n == 1 {
            let single = self.translate(texts[0], domain_hint, vocab_instruction).await?;
            return Ok(Some(vec![single]));
        }

        let hint_lower = domain_hint.to_lowercase();
        let is_xianxia = hint_lower.contains("tu tiên")
            || hint_lower.contains("xianxia")
            || hint_lower.contains("xuanhuan")
            || hint_lower.contains("huyền huyễn")
            || hint_lower.contains("donghua")
            || hint_lower.contains("manhua")
            || hint_lower.contains("cổ trang")
            || hint_lower.contains("kiếm hiệp");

        let is_modern = hint_lower.contains("hiện đại")
            || hint_lower.contains("modern")
            || hint_lower.contains("đô thị")
            || hint_lower.contains("urban");

        let style_directive = if is_xianxia {
            "Translate each numbered Chinese line to Vietnamese in classical Tu Tiên/Xianxia style (STRICTLY use: 我=ta, 你=ngươi, 妻子=thê tử, 丈夫=phu quân, 女人=nữ nhân, 杀戮=sát lục, 修为=tu vi; NEVER use: tôi, bạn, người vợ). Output EXACTLY the same numbered lines (1. ... 2. ...)"
        } else if is_modern {
            "Translate each numbered Chinese line to Vietnamese in modern register. Output EXACTLY the same numbered lines (1. ... 2. ...)"
        } else {
            "Translate each numbered Chinese line to Vietnamese. Output EXACTLY the same numbered lines (1. ... 2. ...)"
        };

        let mut numbered_input = String::new();
        for (i, t) in texts.iter().enumerate() {
            numbered_input.push_str(&format!("{}. {}\n", i + 1, t));
        }

        let prompt = match vocab_instruction {
            Some(v) if !v.is_empty() => format!(
                "{}\n(Terminology: {})\n\nLines to translate:\n{}",
                style_directive, v, numbered_input
            ),
            _ => format!("{}\n\nLines to translate:\n{}", style_directive, numbered_input),
        };

        let batch_tokens = (n as u32 * 90 + 128).min(1800);
        let body = serde_json::json!({
            "messages": [
                { "role": "user", "content": prompt }
            ],
            "temperature": self.config.temperature,
            "max_tokens": batch_tokens
        });

        let url = format!("http://127.0.0.1:{}/v1/chat/completions", self.port);
        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .context("Lỗi kết nối tới Native Hy-MT2 Engine khi dịch batch")?;

        let parsed: ChatCompletionResponse = resp
            .json()
            .await
            .context("Lỗi đọc dữ liệu JSON batch từ Native Hy-MT2 Engine")?;

        let raw = parsed
            .choices
            .first()
            .map(|c| c.message.content.trim().to_string())
            .unwrap_or_default();

        // Phân tích bóc tách các dòng đánh số
        let mut parsed_results: Vec<Option<String>> = vec![None; n];

        for line in raw.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            // Tìm số thứ tự ở đầu dòng (1. hoặc 1: hoặc 1、hoặc [1])
            for target_idx in 1..=n {
                let prefixes = [
                    format!("{}.", target_idx),
                    format!("{}:", target_idx),
                    format!("{}、", target_idx),
                    format!("[{}]", target_idx),
                    format!("({})", target_idx),
                ];

                let mut matched = false;
                for p in &prefixes {
                    if trimmed.starts_with(p) {
                        let text_part = trimmed[p.len()..].trim().trim_matches('"').trim_matches('\'').trim();
                        parsed_results[target_idx - 1] = Some(text_part.to_string());
                        matched = true;
                        break;
                    }
                }
                if matched {
                    break;
                }
            }
        }

        // Kiểm tra xem có đủ đúng n câu không
        let mut final_lines = Vec::with_capacity(n);
        for item in parsed_results {
            if let Some(content) = item {
                if !content.is_empty() {
                    final_lines.push(content);
                } else {
                    return Ok(None); // Dòng rỗng -> fallback
                }
            } else {
                return Ok(None); // Thiếu câu -> fallback
            }
        }

        if final_lines.len() == n {
            Ok(Some(final_lines))
        } else {
            Ok(None)
        }
    }

    /// Gửi truy vấn hoàn thiện hội thoại tổng quát (Dùng cho Tầng 2 Qwen 3.5 và các tác vụ LLM khác)
    pub async fn chat_complete(
        &self,
        system_prompt: Option<&str>,
        user_prompt: &str,
        max_tokens: usize,
    ) -> Result<String> {
        let mut messages = Vec::new();
        if let Some(sys) = system_prompt {
            messages.push(serde_json::json!({ "role": "system", "content": sys }));
        }
        messages.push(serde_json::json!({ "role": "user", "content": user_prompt }));

        let body = serde_json::json!({
            "messages": messages,
            "temperature": self.config.temperature,
            "max_tokens": max_tokens
        });

        let url = format!("http://127.0.0.1:{}/v1/chat/completions", self.port);
        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .context("Lỗi kết nối tới Native Engine qua chat_complete")?;

        let parsed: ChatCompletionResponse = resp
            .json()
            .await
            .context("Lỗi đọc dữ liệu JSON từ Native Engine qua chat_complete")?;

        let raw = parsed
            .choices
            .first()
            .map(|c| c.message.content.trim().to_string())
            .unwrap_or_default();

        Ok(raw)
    }
}

impl Drop for HyMtWorker {
    fn drop(&mut self) {
        // Tự động thu dọn tiến trình C++ khi worker ra khỏi scope (RAII Guard)
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
