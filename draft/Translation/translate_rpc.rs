#[warn(dead_code)]

use anyhow::{bail, Context, Result};
use regex::Regex;
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, ACCEPT_LANGUAGE, REFERER, USER_AGENT};
use serde_json::Value;

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct SubtitleCue {
    pub id: usize,
    pub start: f64,
    pub end: f64,
    pub text: String,
}

pub struct GoogleRpcEngine {
    client: reqwest::Client,
    quote_cleaner: Regex,
    json_fixer: Regex,
}

impl GoogleRpcEngine {
    pub fn new() -> Self {
        let mut headers = HeaderMap::new();
        headers.insert(
            USER_AGENT,
            HeaderValue::from_static(
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36",
            ),
        );
        headers.insert(REFERER, HeaderValue::from_static("https://translate.google.com/"));
        headers.insert(ACCEPT, HeaderValue::from_static("*/*"));
        // Cải thiện Accept-Language để tối ưu hóa bộ giải mã ngữ cảnh của Google
        headers.insert(
            ACCEPT_LANGUAGE,
            HeaderValue::from_static("en-US,en;q=0.9,vi;q=0.8"),
        );

        let client = reqwest::Client::builder()
            .default_headers(headers)
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .expect("Không thể tạo HTTP client");

        let quote_cleaner = Regex::new(r#"["“”„‟«»]"#).unwrap();
        let json_fixer = Regex::new(r#"[“”„‟]"#).unwrap();

        Self {
            client,
            quote_cleaner,
            json_fixer,
        }
    }

    pub fn sanitize_text(&self, text: &str) -> String {
        self.quote_cleaner.replace_all(text, "'").trim().to_string()
    }

    async fn send_rpc_request(&self, payload: &str) -> Result<String> {
        let url = "https://clients5.google.com/translate_a/t";
        let params = [
            ("client", "dict-chrome-ex"),
            ("sl", "en"),
            ("tl", "vi"),
            ("q", payload),
        ];

        let response = self
            .client
            .get(url)
            .query(&params)
            .send()
            .await
            .context("Không thể kết nối đến Google Translate RPC")?;

        if !response.status().is_success() {
            bail!("Google Translate RPC trả về HTTP status: {}", response.status());
        }

        let raw_json: Value = response
            .json()
            .await
            .context("Lỗi parse cấu trúc mảng JSON từ RPC")?;

        if let Some(arr) = raw_json.as_array() {
            if let Some(first) = arr.get(0).and_then(|v| v.as_str()) {
                return Ok(first.to_string());
            }
        }
        if let Some(s) = raw_json.as_str() {
            return Ok(s.to_string());
        }

        Ok(String::new())
    }

    pub async fn translate_single(&self, text: &str) -> Result<String> {
        let clean = self.sanitize_text(text);
        if clean.is_empty() {
            return Ok(String::new());
        }
        self.send_rpc_request(&clean).await
    }

    pub async fn translate_sentences(&self, sentences: &[&str]) -> Result<Vec<String>> {
        if sentences.is_empty() {
            return Ok(Vec::new());
        }

        let chunk_size = 25;
        let mut all_translated = Vec::with_capacity(sentences.len());

        for chunk in sentences.chunks(chunk_size) {
            let clean_chunk: Vec<String> = chunk
                .iter()
                .map(|s| self.sanitize_text(s))
                .collect();

            // Nối các câu bằng ký tự xuống dòng để dịch hàng loạt một request
            let joined_payload = clean_chunk.join("\n");

            let translated_chunk = match self.send_rpc_request(&joined_payload).await {
                Ok(raw_text) => {
                    let lines: Vec<String> = raw_text
                        .split('\n')
                        .map(|l| l.trim().to_string())
                        .collect();

                    if lines.len() == chunk.len() {
                        lines
                    } else {
                        // Fallback: nếu số dòng trả về không khớp, dịch tuần tự từng câu đơn
                        crate::shared::tui_events::emit_log(format!(
                            "   [Translation/RPC] Batch lệch số dòng (nhận {}, cần {}), chuyển sang dịch đơn lẻ...",
                            lines.len(),
                            chunk.len()
                        ));
                        let mut fallback = Vec::with_capacity(chunk.len());
                        for s in &clean_chunk {
                            let single = self.translate_single(s).await
                                .with_context(|| format!("Lỗi dịch câu đơn qua Google Translate: '{}'", s))?;
                            fallback.push(single);
                        }
                        fallback
                    }
                }
                Err(e) => {
                    crate::shared::tui_events::emit_log(format!("   [Translation/RPC] Request batch lỗi ({:?}), chuyển sang dịch từng câu đơn...", e));
                    let mut fallback = Vec::with_capacity(chunk.len());
                    for s in &clean_chunk {
                        let single = self.translate_single(s).await
                            .with_context(|| format!("Lỗi dịch câu đơn qua Google Translate: '{}'", s))?;
                        fallback.push(single);
                    }
                    fallback
                }
            };

            all_translated.extend(translated_chunk);
        }

        Ok(all_translated)
    }
}