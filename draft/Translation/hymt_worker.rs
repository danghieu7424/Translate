/****
 * Module: HyMtWorker
 * Chức năng: Quản lý Native C++ Inference Engine cho mô hình Tencent Hy-MT2 1.8B GGUF.
 * Ranh giới bảo vệ:
 *   - 100% Native C++ (Zero-Python): Không phụ thuộc vào Python runtime, pip, hay py launcher.
 *   - Khởi động local engine độc lập, giao tiếp loopback localhost qua HTTP Chat Completions.
 *   - Tự động đóng tiến trình C++ và giải phóng bộ nhớ RAM ngay khi hoàn thành (RAII Drop).
 ****/

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
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

pub struct HyMtWorker {
    child: Child,
    port: u16,
    client: reqwest::Client,
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

    /// Định vị file thực thi C++ Engine (hymt_server.exe hoặc llama-server.exe)
    fn find_server_binary() -> Result<PathBuf> {
        let mut candidates = Vec::new();

        if let Ok(exe) = std::env::current_exe() {
            if let Some(parent) = exe.parent() {
                // 1. Ưu tiên số 1: Cùng cấp với file exe đang chạy (Production Bundle / Target)
                candidates.push(parent.join("hymt_bin").join("hymt_server.exe"));
                candidates.push(parent.join("hymt_bin").join("llama-server.exe"));
                candidates.push(parent.join("hymt_server.exe"));
                candidates.push(parent.join("llama-server.exe"));

                // 2. Nếu exe nằm trong target/release hoặc target/debug, tìm ngược lên workspace root
                if let Some(target_dir) = parent.parent() {
                    if let Some(ws_root) = target_dir.parent() {
                        candidates.push(ws_root.join("dist").join("aidub_bundle").join("hymt_bin").join("hymt_server.exe"));
                        candidates.push(ws_root.join("dist").join("aidub_bundle").join("hymt_bin").join("llama-server.exe"));
                        candidates.push(ws_root.join("scratch").join("llama_bin").join("llama-server.exe"));
                    }
                }
            }
        }

        // 3. Fallback theo CWD (hỗ trợ cả khi chạy từ workspace root pipline_dubbing hoặc subfolder aidub)
        candidates.push(PathBuf::from("dist/aidub_bundle/hymt_bin/hymt_server.exe"));
        candidates.push(PathBuf::from("dist/aidub_bundle/hymt_bin/llama-server.exe"));
        candidates.push(PathBuf::from("aidub/dist/aidub_bundle/hymt_bin/hymt_server.exe"));
        candidates.push(PathBuf::from("aidub/dist/aidub_bundle/hymt_bin/llama-server.exe"));
        candidates.push(PathBuf::from("scratch/llama_bin/llama-server.exe"));
        candidates.push(PathBuf::from("aidub/scratch/llama_bin/llama-server.exe"));

        for p in candidates {
            if p.exists() {
                return Ok(p);
            }
        }

        Err(anyhow!(
            "Không tìm thấy Native C++ Engine (hymt_server.exe / llama-server.exe). Hãy đảm bảo thư mục hymt_bin nằm cạnh file aidub.exe"
        ))
    }

    /// Khởi động Native C++ Engine và đợi sẵn sàng
    pub async fn new(model_path: &Path) -> Result<Self> {
        let server_bin = Self::find_server_binary()?;
        let port = Self::find_available_port(18080);
        let cpu_threads = std::thread::available_parallelism()
            .map(|n| (n.get().saturating_sub(2)).max(1))
            .unwrap_or(4);

        let mut cmd = Command::new(&server_bin);
        // WHY: Đặt current_dir tại thư mục chứa binary C++ để Windows nạp đúng các DLL đi kèm (llama.dll, libomp.dll...)
        if let Some(bin_dir) = server_bin.parent() {
            cmd.current_dir(bin_dir);
        }

        cmd.arg("-m")
            .arg(model_path)
            .arg("--port")
            .arg(port.to_string())
            .arg("-c")
            .arg("2048")
            .arg("-t")
            .arg(cpu_threads.to_string())
            .arg("--log-disable");

        // Tự động offload 99 layers sang VRAM GPU nếu phát hiện card đồ họa rời đủ điều kiện
        if crate::shared::HardwareDetector::is_gpu_eligible() {
            cmd.arg("-ngl").arg("99");
        }

        let child = cmd
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| format!("Không thể khởi chạy Native C++ Engine tại {:?}", server_bin))?;

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(60))
            .build()?;

        // Health check đợi server nạp mô hình vào RAM
        let t0 = Instant::now();
        let timeout = Duration::from_secs(30);
        let health_url = format!("http://127.0.0.1:{}/health", port);
        let mut ready = false;

        while t0.elapsed() < timeout {
            tokio::time::sleep(Duration::from_millis(250)).await;
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
            return Err(anyhow!("Quá thời gian khởi tạo Native Hy-MT2 Engine (Timeout 30s)"));
        }

        Ok(Self { child, port, client })
    }

    /*****
     * Gửi câu dịch trực tiếp tới Native Engine qua loopback localhost
     * Phân tách ma trận xưng hô cho Tu Tiên vs Hiện Đại / Đô Thị vs Đa dụng
     *****/
    pub async fn translate(&self, _id: usize, text: &str, hint: &str) -> Result<String> {
        let is_chinese = text.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c));
        let hint_lower = hint.to_lowercase();
        let is_xianxia = hint_lower.contains("xianxia")
            || hint_lower.contains("xuanhuan")
            || hint_lower.contains("tu tiên")
            || hint_lower.contains("huyền huyễn")
            || hint_lower.contains("donghua")
            || hint_lower.contains("manhua")
            || hint_lower.contains("fantasy web-novel")
            || hint_lower.contains("cổ trang")
            || hint_lower.contains("kiếm hiệp");

        // WHY: Phân lập ranh giới cho thể loại phim hiện đại/đô thị/ngôn tình để kích hoạt ma trận xưng hô đời sống
        let is_modern = hint_lower.contains("hiện đại")
            || hint_lower.contains("modern")
            || hint_lower.contains("đô thị")
            || hint_lower.contains("urban")
            || hint_lower.contains("ngôn tình")
            || hint_lower.contains("thanh xuân");

        let system_prompt = if is_xianxia {
            if is_chinese {
                "Dịch phụ đề tiếng Trung sau sang tiếng Việt chuẩn văn phong tiểu thuyết Tu Tiên, Huyền Huyễn, Cổ Trang Hán-Việt. BẮT BUỘC xưng hô theo đúng vai vế và tôn ti môn phái, gia tộc, giai cấp, tuổi tác, giới tính: sư huynh/sư đệ/sư tỷ/sư muội, phu quân/nương tử, tỷ tỷ/muội muội, ca ca/đệ đệ, đạo hữu, tiền bối/vãn bối, tại hạ/các hạ, bổn tọa/bản tôn/lão phu/ngươi/ta. TUYỆT ĐỐI KHÔNG dùng đại từ hiện đại: 'chị em', 'anh em', 'bạn bè', 'vợ chồng', 'tôi', 'bạn'. CHỈ xuất kết quả dịch tiếng Việt thô, không thêm giải thích hay dấu ngoặc kép.".to_string()
            } else {
                "Translate the following subtitle into Vietnamese in classical Xianxia/Donghua style. STRICTLY use Sino-Vietnamese pronouns based on rank, sect, age, and gender (sư huynh/sư đệ/sư tỷ/sư muội, phu quân/nương tử, tỷ tỷ/muội muội, ca ca/đệ đệ, đạo hữu, tiền bối/vãn bối, các hạ/tại hạ, bổn tọa/ngươi/ta). NEVER use modern pronouns like 'chị em', 'anh em', 'vợ chồng', 'bạn bè'. Output ONLY the raw Vietnamese translation.".to_string()
            }
        } else if is_modern {
            if is_chinese {
                "Dịch phụ đề tiếng Trung sau sang tiếng Việt chuẩn văn phong phim Hiện Đại, Đô Thị, Ngôn Tình, Đời Sống. BẮT BUỘC xưng hô tự nhiên theo đời sống hiện đại: anh/em, cậu/tớ, tôi/bạn, sếp/nhân viên, tổng tài, bác sĩ, ba mẹ/con. TUYỆT ĐỐI KHÔNG dùng từ xưng hô cổ trang/tu tiên: 'tại hạ', 'các hạ', 'bổn tọa', 'bản tôn', 'phu quân', 'nương tử', 'sư huynh/sư đệ', 'đạo hữu', 'lão phu'. CHỈ xuất kết quả dịch tiếng Việt thô, không thêm giải thích hay dấu ngoặc kép.".to_string()
            } else {
                "Translate the following subtitle into Vietnamese in modern urban/drama/daily life style. STRICTLY use natural modern Vietnamese pronouns (anh/em, cậu/tớ, tôi/bạn, etc.). NEVER use classical or archaic Sino-Vietnamese pronouns like 'tại hạ', 'các hạ', 'phu quân', 'nương tử', 'bổn tọa', 'đạo hữu'. Output ONLY the raw Vietnamese translation.".to_string()
            }
        } else if is_chinese {
            format!(
                "Translate the following Chinese text to Vietnamese. Output ONLY the raw Vietnamese translation without any explanation, markdown, prefixes, quotes or notes. Domain context: {}.",
                hint
            )
        } else {
            format!(
                "Translate the following English text to Vietnamese. Output ONLY the raw Vietnamese translation without any explanation, markdown, prefixes, quotes or notes. Domain context: {}.",
                hint
            )
        };

        let body = serde_json::json!({
            "messages": [
                { "role": "system", "content": system_prompt },
                { "role": "user", "content": text }
            ],
            "temperature": 0.3,
            "max_tokens": 128
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
        // WHY: Tự động dọn sạch tiến trình C++ khi worker ra khỏi scope (RAII Guard)
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
