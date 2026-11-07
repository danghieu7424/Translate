use anyhow::{bail, Result};
use reqwest::header::{HeaderMap, HeaderValue, CONTENT_TYPE};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use crate::shared::terminal_color::TermColorExt;

/*****
 * TermCorrector — Module sửa thuật ngữ chuyên ngành bằng Gemini Flash
 *
 * Chiến lược tối ưu Rate Limit:
 * - Gộp TẤT CẢ cặp (EN, VI) vào MỘT request duy nhất → 1 RPM cho toàn bộ video
 * - Gemini Flash chỉ sửa thuật ngữ sai, KHÔNG dịch lại → output ổn định
 * - Nếu mảng quá lớn (>50 câu), chia chunk và chờ giữa các chunk
 * - Fallback: nếu LLM fail → trả về bản dịch Google nguyên gốc (không mất dữ liệu)
 *****/

const GEMINI_API_BASE: &str = "https://generativelanguage.googleapis.com/v1beta/models";

// Giới hạn số câu mỗi batch để tránh context window overflow
// và giữ format output ổn định
const BATCH_SIZE: usize = 30;

// Khoảng chờ giữa các batch (ms) để tránh rate limit bản Free (RPM < 10)
const BATCH_DELAY_MS: u64 = 6500;

pub struct TermCorrector {
    client: reqwest::Client,
    api_keys: Vec<String>,
    // Chuỗi model dự phòng: nếu model đầu bị 503/429, tự động thử model tiếp theo
    models: Vec<&'static str>,
    // Vị trí API key đang hoạt động tốt nhất (Sticky Active Key)
    current_key_idx: AtomicUsize,
    // Vị trí model đang hoạt động tốt nhất độc lập cho từng API Key (Cách ly chống ô nhiễm chéo)
    key_model_indices: Vec<AtomicUsize>,
}

#[derive(Serialize, Deserialize, Default)]
struct TermCache {
    sentences: HashMap<usize, String>,
}

fn load_term_cache(path: &Path) -> TermCache {
    if let Ok(content) = std::fs::read_to_string(path) {
        if let Ok(cache) = serde_json::from_str(&content) {
            return cache;
        }
    }
    TermCache::default()
}

fn save_term_cache(path: &Path, cache: &TermCache) {
    if let Ok(content) = serde_json::to_string_pretty(cache) {
        let _ = std::fs::write(path, content);
    }
}

// Cấu trúc request body cho Gemini API
#[derive(Serialize, Clone)]
struct GeminiRequest {
    contents: Vec<GeminiContent>,
    #[serde(rename = "generationConfig")]
    generation_config: GenerationConfig,
}

#[derive(Serialize, Clone)]
struct GeminiContent {
    parts: Vec<GeminiPart>,
}

#[derive(Serialize, Clone)]
struct GeminiPart {
    text: String,
}

#[derive(Serialize, Clone)]
struct GenerationConfig {
    temperature: f32,
    #[serde(rename = "maxOutputTokens")]
    max_output_tokens: u32,
    // Buộc output JSON để dễ parse
    #[serde(rename = "responseMimeType")]
    response_mime_type: String,
}

impl TermCorrector {
    pub fn new(api_keys: Vec<String>) -> Self {
        let mut headers = HeaderMap::new();
        headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );

        let client = reqwest::Client::builder()
            .default_headers(headers)
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .expect("Không thể tạo HTTP client cho Gemini");

        let num_keys = api_keys.len();
        let key_model_indices = (0..num_keys.max(1))
            .map(|_| AtomicUsize::new(0))
            .collect();

        Self {
            client,
            api_keys,
            // Thứ tự tối ưu: Toàn bộ dòng 2.5 tiết kiệm chi phí xếp trước, dòng 3.x dự phòng xếp sau
            models: vec![
                "gemini-2.5-flash",
                "gemini-2.5-flash-lite",
                "gemini-2.5-pro",
                "gemini-3.5-flash-lite",
                "gemini-3.8-flash",
                "gemini-3.1-pro-preview",
            ],
            current_key_idx: AtomicUsize::new(0),
            key_model_indices,
        }
    }

    /*****
     * Xây dựng prompt linh hoạt cho Gemini Flash dựa trên domain hint và custom rules
     *****/
    fn build_prompt(
        &self,
        pairs: &[(&str, &str)],
        domain_hint: Option<&str>,
        custom_rules: Option<&str>,
    ) -> String {
        let mut entries = String::new();
        for (i, (src, vi)) in pairs.iter().enumerate() {
            entries.push_str(&format!(
                "  {{\n    \"i\": {},\n    \"src\": {},\n    \"raw_vi\": {}\n  }}",
                i,
                serde_json::to_string(src).unwrap_or_else(|_| format!("{:?}", src)),
                serde_json::to_string(vi).unwrap_or_else(|_| format!("{:?}", vi)),
            ));
            if i < pairs.len() - 1 {
                entries.push_str(",\n");
            }
        }

        let hint_line = if let Some(hint) = domain_hint {
            if !hint.trim().is_empty() {
                format!("\nLĨNH VỰC/NGỮ CẢNH CHỦ ĐẠO CỦA VIDEO: {}\n", hint.trim())
            } else {
                String::new()
            }
        } else {
            String::new()
        };

        let hint_lower = domain_hint.unwrap_or("").to_lowercase();
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

        let base_rules = if is_xianxia {
            r#"QUY TẮC BẮT BUỘC CHO THỂ LOẠI TU TIÊN / HUYỀN HUYỄN / CỔ TRANG / DONGHUA:
1. ĐẠI TỪ NHÂN XƯNG THEO GIAI CẤP, MÔN PHÁI & TÔN TI:
   - TUYỆT ĐỐI CẤM dùng đại từ hiện đại: "chị em", "anh em", "bạn bè", "tôi - bạn", "cậu - tớ", "vợ - chồng", "mày - tao".
   - Quan hệ Môn phái / Đồng môn: Dùng "Sư huynh - Sư đệ - Sư tỷ - Sư muội", "Sư phụ / Sư tôn - Đồ nhi", "Trưởng môn", "Trưởng lão", "Lão tổ".
   - Quan hệ Gia tộc / Huyết thống: Dùng "Tỷ tỷ - Muội muội" (hoặc Tỷ - Muội, CẤM dịch "chị em"), "Huynh trưởng / Ca ca - Đệ đệ" (CẤM dịch "anh em"), "Phụ thân / Cha - Mẫu thân / Nương".
   - Quan hệ Phu thê / Hôn ước / Tình cảm: Dùng "Phu quân / Tướng công - Nương tử / Thê tử" (CẤM dịch "vợ - chồng"), "Đạo lữ".
   - Quan hệ Xã giao / Tu chân giới: Dùng "Đạo hữu", "Tiền bối - Vãn bối", "Tại hạ - Các hạ", "Huynh đệ".
   - Bậc tôn giả / Cường giả / Kẻ thù: Dùng "Bổn tọa", "Bản tôn", "Lão phu", "Tiểu tử / Tiểu bối", "Ngươi - Ta".
   - Phân biệt theo giới tính & tuổi tác: Nữ nhân đoan trang xưng "Thiếp thân / Tiểu nữ / Nương tử", Trưởng bối xưng "Lão phu / Ta" gọi hậu bối là "Tiểu bối / Ngươi".
2. THUẬT NGỮ TU CHÂN HÁN-VIỆT CHUẨN MỰC:
   - Giữ chuẩn thuật ngữ tiên hiệp Hán-Việt: "Trúc Cơ", "Kim Đan", "Nguyên Anh", "Hóa Thần", "Độ Kiếp", "Chân Tiên", "Ma Tôn", "Thần Thức", "Đan Điền", "Đạo Tâm", "Công Pháp", "Tông Môn", "Luyện Khí", "Phế Nhân"...
3. ĐẢM BẢO câu dịch mạch lạc, diễn đạt gãy gọn theo tiết tấu khẩu hình."#
        } else if is_modern {
            r#"QUY TẮC BẮT BUỘC CHO THỂ LOẠI HIỆN ĐẠI / ĐÔ THỊ / NGÔN TÌNH / PHIM NGẮN:
1. ĐẠI TỪ NHÂN XƯNG HIỆN ĐẠI THEO QUAN HỆ & NGỮ CẢNH:
   - TUYỆT ĐỐI CẤM dùng đại từ cổ trang / kiếm hiệp / tu tiên: CẤM dùng "tại hạ", "các hạ", "bổn tọa", "bản tôn", "tiểu tử", "nương tử", "phu quân", "đạo hữu", "sư huynh - sư đệ", "tiểu bối".
   - Quan hệ Tình cảm / Cặp đôi: Dùng "anh - em", "cậu - tớ" (thời đi học/thanh xuân), "tôi - cô / em" (khi mới quen hoặc giữ khoảng cách).
   - Quan hệ Nơi công sở / Đô thị: Xưng hô tự nhiên: "Sếp / Giám đốc / Tổng tài / Chủ tịch - Tôi", "Trợ lý / Thư ký", "anh/chị - em".
   - Quan hệ Gia đình: Dùng "bố / ba - mẹ", "anh trai - em gái", "chị gái - em trai", "ông / bà - cháu" (TUYỆT ĐỐI CẤM dùng "phụ thân", "mẫu thân").
   - Quan hệ Bạn bè / Đối đầu: Dùng "cậu - tớ", "mình - bạn", "mày - tao" (khi căng thẳng, đối đầu).
2. TỪ NGỮ ĐỜI SỐNG ĐÔ THỊ HIỆN ĐẠI:
   - Sử dụng ngôn từ khẩu ngữ tự nhiên, văn minh, phù hợp văn hóa giao tiếp tiếng Việt hiện đại.
   - Giữ chuẩn các chức danh hiện đại: "Tổng tài", "Thiếu gia", "Tiểu thư", "Chủ tịch", "Luật sư", "Bác sĩ"...
3. ĐẢM BẢO câu dịch ngắn gọn, súc tích, vừa vặn với thời lượng nói và khẩu hình thực tế."#
        } else {
            r#"QUY TẮC HIỆU ĐÍNH TỰ ĐỘNG THEO NGỮ CẢNH:
1. Tự động nhận diện chủ đề của video để chọn đại từ nhân xưng và ngữ điệu tự nhiên nhất.
2. Giữ nguyên hoặc dịch chuẩn xác các thuật ngữ chuyên môn quốc tế (DAW, music, tech, v.v.).
3. Đảm bảo tính liên kết mạch lạc giữa các câu liền kề nếu phụ đề bị ngắt vụn giữa chừng.
4. Đảm bảo câu tiếng Việt ngắn gọn, súc tích để vừa vặn với thời lượng nói."#
        };

        let rules_block = match custom_rules {
            Some(rules) if !rules.trim().is_empty() => {
                format!("\n{}\n\nQUY TẮC BỔ SUNG TÙY CHỈNH:\n{}\n", base_rules, rules.trim())
            }
            _ => format!("\n{}\n", base_rules),
        };

        format!(
            r#"Bạn là chuyên gia dịch thuật & hiệu đính phụ đề video đa lĩnh vực.
Nhiệm vụ: Dựa vào câu gốc "src" (tiếng Anh hoặc tiếng Trung) và bản dịch thô "raw_vi", hãy hiệu đính bản tiếng Việt cho chuẩn xác thuật ngữ chuyên ngành và diễn đạt tự nhiên theo đúng vai vế nhân vật.{hint_line}{rules_block}
5. KHÔNG bỏ sót câu, KHÔNG gộp câu. Số lượng phần tử đầu ra PHẢI CHÍNH XÁC bằng số lượng đầu vào ({count}).

INPUT (mảng JSON các cặp câu gốc và bản dịch thô):
[
{entries}
]

OUTPUT: Trả về ĐÚNG một mảng JSON string các câu tiếng Việt đã hiệu đính, giữ nguyên thứ tự index 0 đến {max_idx}.
Ví dụ: ["câu 0", "câu 1", ...]"#,
            hint_line = hint_line,
            rules_block = rules_block,
            entries = entries,
            count = pairs.len(),
            max_idx = pairs.len().saturating_sub(1)
        )
    }

    /*****
     * Gửi 1 batch lên Gemini và parse kết quả
     * Kiên trì chờ (Wait & Retry with Backoff) khi gặp 429 / 503 (Free tier rate limit)
     * Chỉ chuyển model nếu gặp lỗi vĩnh viễn (404, 400, 403) hoặc vượt ngưỡng kiên trì
     *****/
    async fn correct_batch(
        &self,
        pairs: &[(&str, &str)],
        _google_originals: &[&str],
        domain_hint: Option<&str>,
        custom_rules: Option<&str>,
    ) -> Result<Vec<String>> {
        let prompt = self.build_prompt(pairs, domain_hint, custom_rules);

        let request_body = GeminiRequest {
            contents: vec![GeminiContent {
                parts: vec![GeminiPart { text: prompt }],
            }],
            generation_config: GenerationConfig {
                temperature: 0.1,
                max_output_tokens: 8192,
                response_mime_type: "application/json".to_string(),
            },
        };

        let num_keys = self.api_keys.len();
        if num_keys == 0 {
            crate::shared::tui_events::emit_log(format!("{} {} Không có GEMINI_API_KEY nào hợp lệ.", "[TermCorrector]".name_utils().violed(), cli_gui_form::Icons::ERROR));
            anyhow::bail!("Không có GEMINI_API_KEY nào hợp lệ.");
        }

        let mut last_error = String::new();
        let mut had_quota_limit = false;

        // Bắt đầu từ key đang active ở batch trước
        let start_key_idx = self.current_key_idx.load(Ordering::Relaxed) % num_keys;

        // Vòng lặp 1: Duyệt qua các Key trong danh sách
        for key_offset in 0..num_keys {
            let actual_key_idx = (start_key_idx + key_offset) % num_keys;
            let current_key = &self.api_keys[actual_key_idx];

            if key_offset > 0 {
                crate::shared::tui_events::emit_log(format!(
                    "{} {} Đổi sang API Key ({}/{}) sau khi Key trước cạn quota/lỗi...",
                    "[TermCorrector]".name_utils().violed(),
                    cli_gui_form::Icons::ARROW_SWAP,
                    actual_key_idx + 1,
                    num_keys
                ));
            }

            // Dynamic Probing: Mỗi key duy trì độc lập model active của chính nó (Key mới bắt đầu từ 0 để dò 2.5)
            let start_model_idx = self.key_model_indices.get(actual_key_idx)
                .map(|slot| slot.load(Ordering::Relaxed))
                .unwrap_or(0)
                .min(self.models.len().saturating_sub(1));

            // Vòng lặp 2: Duyệt qua danh sách Models của Key này
            for (offset, &initial_model) in self.models[start_model_idx..].iter().enumerate() {
                let actual_model_idx = start_model_idx + offset;
                let mut model = initial_model;
                let mut url = format!(
                    "{}/{}:generateContent?key={}",
                    GEMINI_API_BASE, model, current_key
                );

                let mut attempt: u32 = 0;
                // Nếu có nhiều key, chỉ cần chờ tối đa 4 lần trên mỗi model trước khi chuyển tiếp
                let max_retries = if num_keys > 1 { 4 } else { 12 };

                loop {
                    let result = self.client.post(&url).json(&request_body).send().await;

                    let response = match result {
                        Ok(resp) => resp,
                        Err(e) => {
                            attempt += 1;
                            last_error = format!("[Key {}][{model}] Lỗi kết nối: {e}", actual_key_idx + 1);
                            if attempt >= max_retries {
                                crate::shared::tui_events::emit_log(format!("{} [Key {}][{model}] Quá số lần thử kết nối ({attempt} lần). Chuyển model dự phòng...", "[TermCorrector]".name_utils().violed(), actual_key_idx + 1));
                                if let Some(slot) = self.key_model_indices.get(actual_key_idx) {
                                    slot.store(actual_model_idx + 1, Ordering::Relaxed);
                                }
                                break;
                            }
                            let wait_sec = (attempt * 5).min(30);
                            crate::shared::tui_events::emit_log(format!("{} {} [Key {}][{model}] Lỗi mạng ({e}). Chờ {wait_sec}s rồi thử lại ({attempt}/{max_retries})...", "[TermCorrector]".name_utils().violed(), cli_gui_form::Icons::ARROW_SWAP, actual_key_idx + 1));
                            tokio::time::sleep(std::time::Duration::from_secs(wait_sec as u64)).await;
                            continue;
                        }
                    };

                    let status = response.status();

                    // 1. Rate Limit 429 hoặc Server Bận 503: KIÊN TRÌ CHỜ ĐẾN KHI HỒI PHỤC QUOTA
                    if status.as_u16() == 429 || status.as_u16() == 503 {
                        had_quota_limit = true;
                        attempt += 1;
                        last_error = format!("[Key {}][{model}] HTTP {status}", actual_key_idx + 1);
                        if attempt >= max_retries {
                            crate::shared::tui_events::emit_log(format!(
                                "{} {} [Key {}][{model}] Đã kiên trì chờ {attempt} lần nhưng vẫn bị giới hạn ({status}). Chuyển model/key tiếp theo...",
                                "[TermCorrector]".name_utils().violed(),
                                cli_gui_form::Icons::WARNING,
                                actual_key_idx + 1
                            ));
                            if let Some(slot) = self.key_model_indices.get(actual_key_idx) {
                                slot.store(actual_model_idx + 1, Ordering::Relaxed);
                            }
                            break;
                        }
                        let wait_sec = (attempt * 6).min(35);
                        crate::shared::tui_events::emit_log(format!(
                            "{} {} [Key {}][{model}] Bị giới hạn Free tier (HTTP {status}). Đang chờ {wait_sec}s để hồi phục quota rồi thử lại ({attempt}/{max_retries})...",
                            "[TermCorrector]".name_utils().violed(),
                            cli_gui_form::Icons::HOURGLASS,
                            actual_key_idx + 1
                        ));
                        tokio::time::sleep(std::time::Duration::from_secs(wait_sec as u64)).await;
                        continue; // THỬ LẠI MODEL NÀY
                    }

                    // 2. Lỗi vĩnh viễn (404 Not Found, 400 Bad Request, 403 Forbidden...) -> Bỏ qua model
                    if !status.is_success() {
                        let body = response.text().await.unwrap_or_default();

                        // Xử lý tối ưu: Bắt chính xác lỗi 404 khi Google thông báo model 2.5 không còn hỗ trợ cho New Users
                        // Tin nhắn mẫu: "This model models/gemini-2.5-... is no longer available to new users. Please update your code to use..."
                        if status.as_u16() == 404 && body.contains("is no longer available to new users") {
                            let replacement = if body.contains("gemini-3.5-flash-lite") {
                                "gemini-3.5-flash-lite"
                            } else if body.contains("gemini-3.8-flash") {
                                "gemini-3.8-flash"
                            } else if body.contains("gemini-3.1-pro-preview") {
                                "gemini-3.1-pro-preview"
                            } else {
                                "gemini-3.8-flash"
                            };

                            crate::shared::tui_events::emit_log(format!(
                                "{} {} [Key {}] Google báo model [{model}] không khả dụng cho tài khoản mới. Tự động chuyển tiếp sang [{replacement}]...",
                                "[TermCorrector]".name_utils().violed(),
                                cli_gui_form::Icons::ARROW_SWAP,
                                actual_key_idx + 1
                            ));

                            // Đồng bộ chỉ số model riêng cho key này để các batch tiếp theo của key này gọi thẳng model 3.x
                            if let Some(pos) = self.models.iter().position(|&m| m == replacement) {
                                if let Some(slot) = self.key_model_indices.get(actual_key_idx) {
                                    slot.store(pos, Ordering::Relaxed);
                                }
                            }

                            url = format!(
                                "{}/{}:generateContent?key={}",
                                GEMINI_API_BASE, replacement, current_key
                            );
                            model = replacement;
                            attempt = 0;
                            continue;
                        }

                        last_error = format!("[Key {}][{model}] HTTP {status}: {body}", actual_key_idx + 1);
                        crate::shared::tui_events::emit_log(format!("{} {} [Key {}][{model}] Lỗi cấu hình/vĩnh viễn ({status}). Bỏ qua model này...", "[TermCorrector]".name_utils().violed(), cli_gui_form::Icons::ERROR, actual_key_idx + 1));
                        if let Some(slot) = self.key_model_indices.get(actual_key_idx) {
                            slot.store(actual_model_idx + 1, Ordering::Relaxed);
                        }
                        break;
                    }

                    // 3. Phân tích kết quả trả về từ Gemini
                    let raw: Value = match response.json().await {
                        Ok(v) => v,
                        Err(e) => {
                            last_error = format!("[Key {}][{model}] Lỗi parse JSON response: {e}", actual_key_idx + 1);
                            attempt += 1;
                            if attempt >= max_retries {
                                break;
                            }
                            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                            continue;
                        }
                    };

                    let text_output = raw
                        .get("candidates")
                        .and_then(|c| c.get(0))
                        .and_then(|c| c.get("content"))
                        .and_then(|c| c.get("parts"))
                        .and_then(|p| p.get(0))
                        .and_then(|p| p.get("text"))
                        .and_then(|t| t.as_str())
                        .unwrap_or("");

                    if text_output.is_empty() {
                        last_error = format!("[Key {}][{model}] Gemini trả về nội dung rỗng", actual_key_idx + 1);
                        attempt += 1;
                        if attempt >= max_retries {
                            break;
                        }
                        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                        continue;
                    }

                    let trimmed = text_output.trim();
                    let clean_json = if let Some(stripped) = trimmed.strip_prefix("```json") {
                        stripped.strip_suffix("```").unwrap_or(stripped).trim()
                    } else if let Some(stripped) = trimmed.strip_prefix("```") {
                        stripped.strip_suffix("```").unwrap_or(stripped).trim()
                    } else {
                        trimmed
                    };

                    let corrected: Vec<String> = match serde_json::from_str(clean_json) {
                        Ok(arr) => arr,
                        Err(e) => {
                            last_error = format!("[Key {}][{model}] Lỗi parse mảng JSON: {e}", actual_key_idx + 1);
                            attempt += 1;
                            if attempt >= max_retries {
                                break;
                            }
                            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                            continue;
                        }
                    };

                    if corrected.len() != pairs.len() {
                        last_error = format!(
                            "[Key {}][{model}] Lệch số lượng câu! Input: {}, Output: {}",
                            actual_key_idx + 1,
                            pairs.len(),
                            corrected.len()
                        );
                        attempt += 1;
                        if attempt >= max_retries {
                            break;
                        }
                        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                        continue;
                    }

                    // Ghi nhận Key và Model riêng của key này đang hoạt động tốt cho các batch tiếp theo
                    self.current_key_idx.store(actual_key_idx, Ordering::Relaxed);
                    let active_idx = self.models.iter().position(|&m| m == model).unwrap_or(actual_model_idx);
                    if let Some(slot) = self.key_model_indices.get(actual_key_idx) {
                        slot.store(active_idx, Ordering::Relaxed);
                    }
                    if num_keys > 1 {
                        crate::shared::tui_events::emit_log(format!("{} {} Thành công với Key {}/{} [{model}].", "[TermCorrector]".name_utils().violed(), cli_gui_form::Icons::SUCCESS, actual_key_idx + 1, num_keys));
                    } else {
                        crate::shared::tui_events::emit_log(format!("{} {} Thành công với model [{model}].", "[TermCorrector]".name_utils().violed(), cli_gui_form::Icons::SUCCESS));
                    }
                    return Ok(corrected);
                }
            }

            // Nếu đã hết các model của key hiện tại mà vẫn thất bại -> reset model của key này về 0 để lần sau dò lại
            if let Some(slot) = self.key_model_indices.get(actual_key_idx) {
                slot.store(0, Ordering::Relaxed);
            }
        }

        if had_quota_limit {
            crate::shared::tui_events::emit_log(format!(
                "{} {} Tất cả {} key và các model đều cạn Quota (429/503). Lỗi cuối: {last_error}.",
                "[TermCorrector]".name_utils().violed(),
                cli_gui_form::Icons::HOURGLASS,
                num_keys
            ));
            anyhow::bail!("QUOTA_EXHAUSTED: Toàn bộ {} API key và model đều cạn Quota. Lỗi: {}", num_keys, last_error);
        } else {
            crate::shared::tui_events::emit_log(format!(
                "{} Tất cả {} key và các model đều thất bại. Lỗi cuối: {last_error}.",
                "[TermCorrector]".name_utils().violed(),
                num_keys
            ));
            anyhow::bail!("GEMINI_ERROR: {}", last_error);
        }
    }

    /*****
     * Entry point: nhận mảng EN gốc + mảng VI từ Google Translate
     *
     * Chia thành các batch <= BATCH_SIZE câu, gửi tuần tự với delay giữa
     * các batch để tôn trọng rate limit. Tổng RPM tối đa = ceil(N / BATCH_SIZE).
     *
     * Ví dụ: 200 subtitle lines → 200/30 = 7 requests → dưới 1 phút
     *****/
    pub async fn correct_terms(
        &self,
        en_sentences: &[&str],
        vi_sentences: &[String],
        cache_path: Option<&Path>,
        domain_hint: Option<&str>,
        custom_rules: Option<&str>,
    ) -> Result<Vec<String>> {
        if en_sentences.len() != vi_sentences.len() {
            bail!(
                "Số câu EN ({}) không khớp VI ({})",
                en_sentences.len(),
                vi_sentences.len()
            );
        }

        let total = en_sentences.len();
        let mut all_corrected: Vec<String> = Vec::with_capacity(total);

        // Nạp checkpoint cache nếu có sẵn
        let mut cache = cache_path.map(load_term_cache).unwrap_or_default();

        // Chia batch để tránh context window overflow và rate limit
        let num_batches = (total + BATCH_SIZE - 1) / BATCH_SIZE;

        for batch_idx in 0..num_batches {
            let start = batch_idx * BATCH_SIZE;
            let end = (start + BATCH_SIZE).min(total);

            // Kiểm tra xem tất cả các câu trong batch này đã có trong cache chưa
            let all_in_cache = (start..end).all(|i| cache.sentences.contains_key(&i));

            if all_in_cache {
                crate::shared::tui_events::emit_log(format!(
                    "{} Batch {}/{}: Đã phục hồi từ cache checkpoint (câu {}-{})...",
                    "[TermCorrector]".name_utils().violed(),
                    batch_idx + 1,
                    num_batches,
                    start + 1,
                    end
                ));
                for i in start..end {
                    if let Some(cached_text) = cache.sentences.get(&i) {
                        all_corrected.push(cached_text.clone());
                    }
                }
                continue;
            }

            let pairs: Vec<(&str, &str)> = (start..end)
                .map(|i| (en_sentences[i], vi_sentences[i].as_str()))
                .collect();

            let google_originals: Vec<&str> = (start..end)
                .map(|i| vi_sentences[i].as_str())
                .collect();

            crate::shared::tui_events::emit_log(format!(
                "{} Batch {}/{}: sửa thuật ngữ câu {}-{}...",
                "[TermCorrector]".name_utils().violed(),
                batch_idx + 1,
                num_batches,
                start + 1,
                end
            ));

            // Chỉ khi Gemini thành công mới nhận batch_result, nếu hết quota sẽ ném lỗi ngay
            let batch_result = self.correct_batch(&pairs, &google_originals, domain_hint, custom_rules).await?;

            // Lưu kết quả vào checkpoint cache ngay lập tức sau mỗi batch (đảm bảo không bị ô nhiễm cache)
            for (idx_in_batch, text) in batch_result.iter().enumerate() {
                cache.sentences.insert(start + idx_in_batch, text.clone());
            }
            if let Some(cp) = cache_path {
                save_term_cache(cp, &cache);
            }

            all_corrected.extend(batch_result);

            // Chờ giữa các batch để tránh rate limit (trừ batch cuối)
            if batch_idx < num_batches - 1 {
                crate::shared::tui_events::emit_log(format!(
                    "{} Chờ {}ms trước batch tiếp...",
                    "[TermCorrector]".name_utils().violed(),
                    BATCH_DELAY_MS
                ));
                tokio::time::sleep(std::time::Duration::from_millis(BATCH_DELAY_MS)).await;
            }
        }

        Ok(all_corrected)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /****
     * Kiểm thử tính đúng đắn và cô lập ranh giới của prompt generator
     * Ranh giới bảo vệ: Tu Tiên vs Hiện Đại vs Mặc Định không bị xung đột hoặc rò rỉ rule
     ****/
    #[test]
    fn test_domain_hint_isolation() {
        let corrector = TermCorrector::new(vec!["dummy_key".to_string()]);
        let sample_pairs = [("Hello", "Xin chào")];

        // 1. Kiểm thử Xianxia / Tu Tiên
        let xianxia_prompt = corrector.build_prompt(&sample_pairs, Some("tu tiên, xianxia"), None);
        assert!(xianxia_prompt.contains("QUY TẮC BẮT BUỘC CHO THỂ LOẠI TU TIÊN / HUYỀN HUYỄN"));
        assert!(!xianxia_prompt.contains("QUY TẮC BẮT BUỘC CHO THỂ LOẠI HIỆN ĐẠI"));

        // 2. Kiểm thử Hiện Đại / Đô Thị
        let modern_prompt = corrector.build_prompt(
            &sample_pairs,
            Some("modern drama & urban romance (phim hiện đại, đô thị, ngôn tình)"),
            None,
        );
        assert!(modern_prompt.contains("QUY TẮC BẮT BUỘC CHO THỂ LOẠI HIỆN ĐẠI / ĐÔ THỊ / NGÔN TÌNH"));
        assert!(!modern_prompt.contains("QUY TẮC BẮT BUỘC CHO THỂ LOẠI TU TIÊN / HUYỀN HUYỄN"));

        // 3. Kiểm thử Thể loại Mặc định / Chung
        let default_prompt = corrector.build_prompt(&sample_pairs, Some("music production, DAW"), None);
        assert!(default_prompt.contains("QUY TẮC HIỆU ĐÍNH TỰ ĐỘNG THEO NGỮ CẢNH"));
        assert!(!default_prompt.contains("QUY TẮC BẮT BUỘC CHO THỂ LOẠI TU TIÊN"));
        assert!(!default_prompt.contains("QUY TẮC BẮT BUỘC CHO THỂ LOẠI HIỆN ĐẠI"));
    }
}
