//! Async client for OpenAI-compatible local chat-completion APIs.

use reqwest::{Client, Url};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;
use thiserror::Error;

const MIN_REQUEST_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Debug, Clone)]
pub struct ClientConfig {
    pub base_url: String,
    pub request_timeout: Duration,
    pub connect_timeout: Duration,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            base_url: "http://127.0.0.1:8080".to_owned(),
            request_timeout: Duration::from_secs(600),
            connect_timeout: Duration::from_secs(15),
        }
    }
}

#[derive(Debug, Error)]
pub enum LlmClientError {
    #[error("invalid client configuration: {0}")]
    InvalidConfig(String),
    #[error("invalid chat request: {0}")]
    InvalidRequest(String),
    #[error("HTTP request failed: {0}")]
    Transport(#[from] reqwest::Error),
    #[error("local server returned HTTP {status}: {body}")]
    HttpStatus { status: u16, body: String },
    #[error("invalid chat-completion response JSON: {0}")]
    ResponseJson(#[from] serde_json::Error),
    #[error("chat-completion response contains no choices")]
    NoChoices,
    #[error("first chat-completion choice has no text content")]
    MissingContent,
    #[error("first chat-completion choice has empty text content")]
    EmptyContent,
}

#[derive(Debug, Clone)]
pub struct LlmClient {
    http: Client,
    endpoint: String,
}

impl LlmClient {
    pub fn new(config: ClientConfig) -> Result<Self, LlmClientError> {
        if config.request_timeout < MIN_REQUEST_TIMEOUT {
            return Err(LlmClientError::InvalidConfig(
                "request_timeout must be at least 300 seconds".to_owned(),
            ));
        }
        if config.connect_timeout.is_zero() || config.connect_timeout > config.request_timeout {
            return Err(LlmClientError::InvalidConfig(
                "connect_timeout must be greater than zero and no longer than request_timeout"
                    .to_owned(),
            ));
        }

        let endpoint = chat_completions_endpoint(&config.base_url)?;
        let http = Client::builder()
            .timeout(config.request_timeout)
            .connect_timeout(config.connect_timeout)
            .redirect(reqwest::redirect::Policy::none())
            .build()?;

        Ok(Self { http, endpoint })
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub async fn complete(&self, request: &ChatRequest) -> Result<ChatCompletion, LlmClientError> {
        request.validate()?;

        let response = self.http.post(&self.endpoint).json(request).send().await?;
        let status = response.status();
        let body = response.bytes().await?;
        if !status.is_success() {
            return Err(LlmClientError::HttpStatus {
                status: status.as_u16(),
                body: String::from_utf8_lossy(&body).into_owned(),
            });
        }

        let response: ApiResponse = serde_json::from_slice(&body)?;
        let choice = response
            .choices
            .into_iter()
            .next()
            .ok_or(LlmClientError::NoChoices)?;
        let content = choice
            .message
            .content
            .ok_or(LlmClientError::MissingContent)?;
        if content.trim().is_empty() {
            return Err(LlmClientError::EmptyContent);
        }

        Ok(ChatCompletion {
            content,
            finish_reason: choice.finish_reason,
            prompt_tokens: response
                .usage
                .as_ref()
                .and_then(|usage| usage.prompt_tokens),
            completion_tokens: response.usage.and_then(|usage| usage.completion_tokens),
        })
    }
}

fn chat_completions_endpoint(base_url: &str) -> Result<String, LlmClientError> {
    let mut url = Url::parse(base_url).map_err(|error| {
        LlmClientError::InvalidConfig(format!("base_url is not a valid URL: {error}"))
    })?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(LlmClientError::InvalidConfig(
            "base_url must be an http(s) URL without credentials, query, or fragment".to_owned(),
        ));
    }

    let path = url.path().trim_end_matches('/');
    match path {
        "" | "/" | "/v1" => url.set_path("/v1/chat/completions"),
        "/v1/chat/completions" => {}
        _ => {
            return Err(LlmClientError::InvalidConfig(
                "base_url path must be empty, /v1, or /v1/chat/completions".to_owned(),
            ));
        }
    }
    Ok(url.to_string())
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
}

impl ChatMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: content.into(),
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: content.into(),
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: content.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    System,
    User,
    Assistant,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SamplingParams {
    pub temperature: f32,
    pub top_p: f32,
    pub repeat_penalty: f32,
    pub frequency_penalty: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
}

impl Default for SamplingParams {
    fn default() -> Self {
        Self {
            temperature: 0.15,
            top_p: 0.9,
            repeat_penalty: 1.15,
            frequency_penalty: 0.2,
            max_tokens: None,
        }
    }
}

impl SamplingParams {
    fn validate(&self) -> Result<(), LlmClientError> {
        if !self.temperature.is_finite() || !(0.0..=2.0).contains(&self.temperature) {
            return Err(LlmClientError::InvalidRequest(
                "temperature must be between 0 and 2".to_owned(),
            ));
        }
        if !self.top_p.is_finite() || self.top_p <= 0.0 || self.top_p > 1.0 {
            return Err(LlmClientError::InvalidRequest(
                "top_p must be greater than 0 and no greater than 1".to_owned(),
            ));
        }
        if !self.repeat_penalty.is_finite() || self.repeat_penalty <= 0.0 {
            return Err(LlmClientError::InvalidRequest(
                "repeat_penalty must be greater than 0".to_owned(),
            ));
        }
        if !self.frequency_penalty.is_finite() || !(-2.0..=2.0).contains(&self.frequency_penalty) {
            return Err(LlmClientError::InvalidRequest(
                "frequency_penalty must be between -2 and 2".to_owned(),
            ));
        }
        if self.max_tokens == Some(0) {
            return Err(LlmClientError::InvalidRequest(
                "max_tokens must be greater than 0".to_owned(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResponseFormat {
    JsonObject,
    JsonSchema { json_schema: JsonSchemaDefinition },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct JsonSchemaDefinition {
    pub name: String,
    pub strict: bool,
    pub schema: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    #[serde(flatten)]
    pub sampling: SamplingParams,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_format: Option<ResponseFormat>,
}

impl ChatRequest {
    pub fn new(model: impl Into<String>, messages: Vec<ChatMessage>) -> Self {
        Self {
            model: model.into(),
            messages,
            sampling: SamplingParams::default(),
            response_format: None,
        }
    }

    fn validate(&self) -> Result<(), LlmClientError> {
        if self.model.trim().is_empty() {
            return Err(LlmClientError::InvalidRequest(
                "model must not be empty".to_owned(),
            ));
        }
        if self.messages.is_empty() {
            return Err(LlmClientError::InvalidRequest(
                "at least one chat message is required".to_owned(),
            ));
        }
        self.sampling.validate()?;

        if let Some(ResponseFormat::JsonSchema { json_schema }) = &self.response_format {
            if json_schema.name.trim().is_empty() {
                return Err(LlmClientError::InvalidRequest(
                    "JSON schema name must not be empty".to_owned(),
                ));
            }
            if !json_schema.schema.is_object() {
                return Err(LlmClientError::InvalidRequest(
                    "JSON schema must be an object".to_owned(),
                ));
            }
        }

        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatCompletion {
    pub content: String,
    pub finish_reason: Option<String>,
    pub prompt_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
}

#[derive(Deserialize)]
struct ApiResponse {
    choices: Vec<ApiChoice>,
    usage: Option<ApiUsage>,
}

#[derive(Deserialize)]
struct ApiChoice {
    message: ApiMessage,
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct ApiMessage {
    content: Option<String>,
}

#[derive(Deserialize)]
struct ApiUsage {
    prompt_tokens: Option<u64>,
    completion_tokens: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc::{self, Receiver};
    use std::thread::{self, JoinHandle};

    fn mock_server(
        status: &str,
        response_body: &str,
    ) -> (String, Receiver<String>, JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response_body}",
            response_body.len()
        );
        let (sender, receiver) = mpsc::channel();

        let thread = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = Vec::new();
            let mut buffer = [0; 4096];

            loop {
                let count = stream.read(&mut buffer).unwrap();
                if count == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..count]);

                let Some(header_end) = request.windows(4).position(|window| window == b"\r\n\r\n")
                else {
                    continue;
                };
                let header_end = header_end + 4;
                let headers = String::from_utf8_lossy(&request[..header_end]);
                let content_length = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().unwrap())
                    })
                    .unwrap();
                if request.len() >= header_end + content_length {
                    sender
                        .send(
                            String::from_utf8_lossy(
                                &request[header_end..header_end + content_length],
                            )
                            .into_owned(),
                        )
                        .unwrap();
                    break;
                }
            }

            stream.write_all(response.as_bytes()).unwrap();
        });

        (format!("http://{address}"), receiver, thread)
    }

    fn config(base_url: String) -> ClientConfig {
        ClientConfig {
            base_url,
            request_timeout: Duration::from_secs(300),
            ..ClientConfig::default()
        }
    }

    #[tokio::test]
    async fn sends_sampling_parameters_and_strict_json_schema() {
        let response_body = r#"{"choices":[{"message":{"role":"assistant","content":"{\"edits\":[]}"},"finish_reason":"stop"}],"usage":{"prompt_tokens":12,"completion_tokens":3}}"#;
        let (base_url, request_body, server) = mock_server("200 OK", response_body);
        let expected_endpoint = format!("{base_url}/v1/chat/completions");
        let client = LlmClient::new(config(base_url)).unwrap();
        let mut request = ChatRequest::new(
            "qwen-local",
            vec![
                ChatMessage::system("Return JSON"),
                ChatMessage::user("Review"),
            ],
        );
        request.sampling = SamplingParams {
            temperature: 0.15,
            top_p: 0.9,
            repeat_penalty: 1.15,
            frequency_penalty: 0.2,
            max_tokens: Some(512),
        };
        request.response_format = Some(ResponseFormat::JsonSchema {
            json_schema: JsonSchemaDefinition {
                name: "cue_edits".to_owned(),
                strict: true,
                schema: json!({
                    "type": "object",
                    "properties": {"edits": {"type": "array"}},
                    "required": ["edits"],
                    "additionalProperties": false
                }),
            },
        });

        let completion = client.complete(&request).await.unwrap();
        let body: Value = serde_json::from_str(&request_body.recv().unwrap()).unwrap();
        server.join().unwrap();

        assert_eq!(client.endpoint(), expected_endpoint);
        assert_eq!(completion.content, r#"{"edits":[]}"#);
        assert_eq!(completion.finish_reason.as_deref(), Some("stop"));
        assert_eq!(completion.prompt_tokens, Some(12));
        assert_eq!(completion.completion_tokens, Some(3));
        assert_eq!(body["temperature"], 0.15);
        assert_eq!(body["top_p"], 0.9);
        assert_eq!(body["repeat_penalty"], 1.15);
        assert_eq!(body["frequency_penalty"], 0.2);
        assert_eq!(body["max_tokens"], 512);
        assert_eq!(body["response_format"]["type"], "json_schema");
        assert!(
            body["response_format"]["json_schema"]["strict"]
                .as_bool()
                .unwrap()
        );
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][1]["role"], "user");
    }

    #[tokio::test]
    async fn returns_server_status_and_body_instead_of_hiding_errors() {
        let (base_url, _request_body, server) =
            mock_server("400 Bad Request", r#"{"error":"unknown field"}"#);
        let client = LlmClient::new(config(base_url)).unwrap();
        let error = client
            .complete(&ChatRequest::new("local", vec![ChatMessage::user("hello")]))
            .await
            .unwrap_err();
        server.join().unwrap();

        assert!(matches!(
            error,
            LlmClientError::HttpStatus { status: 400, ref body }
                if body.contains("unknown field")
        ));
    }

    #[tokio::test]
    async fn reports_malformed_and_empty_chat_responses() {
        let (base_url, _request_body, server) = mock_server("200 OK", "not-json");
        let client = LlmClient::new(config(base_url)).unwrap();
        let request = ChatRequest::new("local", vec![ChatMessage::user("hello")]);
        assert!(matches!(
            client.complete(&request).await,
            Err(LlmClientError::ResponseJson(_))
        ));
        server.join().unwrap();

        let (base_url, _request_body, server) = mock_server("200 OK", r#"{"choices":[]}"#);
        let client = LlmClient::new(config(base_url)).unwrap();
        assert!(matches!(
            client.complete(&request).await,
            Err(LlmClientError::NoChoices)
        ));
        server.join().unwrap();
    }

    #[test]
    fn timeout_defaults_to_six_hundred_seconds_and_rejects_shorter_values() {
        assert_eq!(
            ClientConfig::default().request_timeout,
            Duration::from_secs(600)
        );

        let mut short_timeout = config("http://127.0.0.1:8080".to_owned());
        short_timeout.request_timeout = Duration::from_secs(299);
        assert!(matches!(
            LlmClient::new(short_timeout),
            Err(LlmClientError::InvalidConfig(_))
        ));
    }

    #[test]
    fn rejects_invalid_sampling_before_making_a_request() {
        let client = LlmClient::new(config("http://127.0.0.1:8080".to_owned())).unwrap();
        let mut request = ChatRequest::new("local", vec![ChatMessage::user("hello")]);
        request.sampling.repeat_penalty = 0.0;

        let runtime = tokio::runtime::Runtime::new().unwrap();
        let error = runtime.block_on(client.complete(&request)).unwrap_err();
        assert!(matches!(error, LlmClientError::InvalidRequest(_)));
    }

    #[test]
    fn response_format_serializes_json_object_and_json_schema_shapes() {
        assert_eq!(
            serde_json::to_value(ResponseFormat::JsonObject).unwrap(),
            json!({"type": "json_object"})
        );
        assert_eq!(
            serde_json::to_value(ResponseFormat::JsonSchema {
                json_schema: JsonSchemaDefinition {
                    name: "example".to_owned(),
                    strict: true,
                    schema: json!({"type": "object"})
                }
            })
            .unwrap(),
            json!({
                "type": "json_schema",
                "json_schema": {
                    "name": "example",
                    "strict": true,
                    "schema": {"type": "object"}
                }
            })
        );
    }

    #[test]
    fn accepts_common_openai_base_url_forms() {
        assert_eq!(
            chat_completions_endpoint("http://localhost:11434").unwrap(),
            "http://localhost:11434/v1/chat/completions"
        );
        assert_eq!(
            chat_completions_endpoint("http://localhost:11434/v1/").unwrap(),
            "http://localhost:11434/v1/chat/completions"
        );
        assert_eq!(
            chat_completions_endpoint("http://localhost:11434/v1/chat/completions").unwrap(),
            "http://localhost:11434/v1/chat/completions"
        );
    }
}
