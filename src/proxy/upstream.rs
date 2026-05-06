#![allow(dead_code)]
use std::sync::Arc;
use std::time::Duration;

use axum::http::HeaderMap;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::ProxyConfig;
use crate::models::upstream::{
    UpstreamConfig, UpstreamGroup, API_TYPE_ANTHROPIC, API_TYPE_OLLAMA, STRATEGY_PRIORITY,
    STRATEGY_ROUND_ROBIN, STRATEGY_WEIGHTED,
};
use crate::utils::error::{AppError, AppResult};

const PASSTHROUGH_HEADER_ALLOWLIST: &[&str] = &[
    "accept",
    "accept-language",
    "accept-charset",
    "user-agent",
    "x-request-id",
    "x-correlation-id",
    "traceparent",
    "tracestate",
    "baggage",
    "openai-organization",
    "openai-project",
    "anthropic-version",
    "anthropic-beta",
];

pub fn estimate_tokens_by_rules(text: &str) -> i64 {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return 0;
    }

    let mut chinese_chars = 0i64;
    let mut english_words = 0i64;
    let mut punctuation_chars = 0i64;
    let mut other_chars = 0i64;
    let mut in_english_word = false;

    for ch in trimmed.chars() {
        if is_english_word_char(ch) {
            if !in_english_word {
                english_words += 1;
                in_english_word = true;
            }
            continue;
        }

        in_english_word = false;

        if ch.is_whitespace() {
            continue;
        }
        if is_cjk_char(ch) {
            chinese_chars += 1;
            continue;
        }
        if is_punctuation_char(ch) {
            punctuation_chars += 1;
            continue;
        }
        other_chars += 1;
    }

    let estimated = chinese_chars as f64
        + english_words as f64 * 2.5
        + punctuation_chars as f64
        + other_chars as f64;

    estimated.ceil() as i64
}

fn is_english_word_char(ch: char) -> bool {
    ch.is_ascii_alphabetic()
}

fn is_cjk_char(ch: char) -> bool {
    ('\u{4E00}'..='\u{9FFF}').contains(&ch)
        || ('\u{3400}'..='\u{4DBF}').contains(&ch)
        || ('\u{F900}'..='\u{FAFF}').contains(&ch)
}

fn is_punctuation_char(ch: char) -> bool {
    ch.is_ascii_punctuation()
        || matches!(
            ch,
            '，'
                | '。'
                | '！'
                | '？'
                | '；'
                | '：'
                | '、'
                | '“'
                | '”'
                | '‘'
                | '’'
                | '（'
                | '）'
                | '【'
                | '】'
                | '《'
                | '》'
                | '「'
                | '」'
                | '『'
                | '』'
                | '—'
                | '…'
                | '·'
                | '～'
                | '﹏'
                | '－'
        )
}

#[derive(Debug, Clone)]
pub struct UpstreamClient {
    client: Client,
}

impl UpstreamClient {
    pub fn new(config: &ProxyConfig) -> AppResult<Self> {
        let client = Client::builder()
            .timeout(Duration::from_secs(config.request_timeout_secs))
            .connect_timeout(Duration::from_secs(config.connect_timeout_secs))
            .pool_max_idle_per_host(config.max_idle_connections)
            .no_proxy()
            .build()
            .map_err(|e| AppError::Internal(format!("Failed to create HTTP client: {}", e)))?;

        Ok(Self { client })
    }

    pub fn http_client(&self) -> &Client {
        &self.client
    }

    pub async fn proxy_request(
        &self,
        upstream: &UpstreamConfig,
        path: &str,
        method: reqwest::Method,
        body: Option<Value>,
        client_headers: &HeaderMap,
        model_headers: Option<&serde_json::Value>,
    ) -> AppResult<reqwest::Response> {
        let url = format!("{}{}", upstream.base_url.trim_end_matches('/'), path);

        let mut request = match method {
            reqwest::Method::GET => self.client.get(&url),
            reqwest::Method::POST => self.client.post(&url),
            reqwest::Method::PUT => self.client.put(&url),
            reqwest::Method::DELETE => self.client.delete(&url),
            reqwest::Method::PATCH => self.client.patch(&url),
            _ => {
                return Err(AppError::BadRequest(format!(
                    "Unsupported method: {}",
                    method
                )))
            }
        };

        for (name, value) in client_headers {
            let name_str = name.as_str();
            if !is_allowed_passthrough_header(name_str) {
                continue;
            }
            if let Ok(value_str) = value.to_str() {
                request = request.header(name_str, value_str);
            }
        }

        if !upstream.api_key_encrypted.is_empty() {
            if is_anthropic_api(upstream) {
                request = request.header("x-api-key", &upstream.api_key_encrypted);
            } else {
                request = request.header(
                    "Authorization",
                    format!("Bearer {}", upstream.api_key_encrypted),
                );
            }
        }
        request = request.header("Content-Type", "application/json");

        if is_anthropic_api(upstream) {
            request = request.header("anthropic-version", "2023-06-01");
        }

        if let Some(headers) = upstream.custom_headers.as_object() {
            for (name, value) in headers {
                if let Some(text) = value.as_str() {
                    request = request.header(name, text);
                } else {
                    request = request.header(name, value.to_string());
                }
            }
        }

        if let Some(headers) = model_headers.and_then(|v| v.as_object()) {
            for (name, value) in headers {
                if let Some(text) = value.as_str() {
                    request = request.header(name, text);
                } else {
                    request = request.header(name, value.to_string());
                }
            }
        }

        if let Some(body) = body {
            request = request.json(&body);
        }

        request = request.header("Accept-Encoding", "identity");

        let response = request.send().await?;

        Ok(response)
    }
}

fn is_allowed_passthrough_header(name: &str) -> bool {
    PASSTHROUGH_HEADER_ALLOWLIST
        .iter()
        .any(|allowed| allowed.eq_ignore_ascii_case(name))
}

#[derive(Debug, Clone)]
pub struct LoadBalancer {
    clients: Arc<DashMap<Uuid, usize>>,
}

use dashmap::DashMap;
use uuid::Uuid;

impl LoadBalancer {
    pub fn new() -> Self {
        Self {
            clients: Arc::new(DashMap::new()),
        }
    }

    pub fn select_upstream(
        &self,
        group: &UpstreamGroup,
        upstreams: &[UpstreamConfig],
    ) -> Option<UpstreamConfig> {
        if upstreams.is_empty() {
            return None;
        }

        match group.balance_strategy.as_str() {
            STRATEGY_PRIORITY => self.select_by_priority(upstreams),
            STRATEGY_ROUND_ROBIN => self.select_round_robin(group.id.into(), upstreams),
            STRATEGY_WEIGHTED => self.select_by_weight(upstreams),
            _ => self.select_by_priority(upstreams),
        }
    }

    fn select_by_priority(&self, upstreams: &[UpstreamConfig]) -> Option<UpstreamConfig> {
        upstreams.first().cloned()
    }

    fn select_round_robin(
        &self,
        group_id: Uuid,
        upstreams: &[UpstreamConfig],
    ) -> Option<UpstreamConfig> {
        if upstreams.is_empty() {
            return None;
        }

        let mut idx = self.clients.entry(group_id).or_insert(0);
        let current = *idx;
        *idx = (*idx + 1) % upstreams.len();

        upstreams.get(current).cloned()
    }

    fn select_by_weight(&self, upstreams: &[UpstreamConfig]) -> Option<UpstreamConfig> {
        use rand::Rng;

        let total_weight: i32 = upstreams.iter().map(|u| u.weight.max(1)).sum();
        if total_weight == 0 {
            return upstreams.first().cloned();
        }
        let mut rng = rand::thread_rng();
        let mut random = rng.gen_range(0..total_weight);

        for upstream in upstreams {
            random -= upstream.weight.max(1);
            if random < 0 {
                return Some(upstream.clone());
            }
        }

        upstreams.last().cloned()
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ChatCompletionRequest {
    pub model: String,
    pub messages: Vec<Message>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_completion_tokens: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
}

impl ChatCompletionRequest {
    pub fn effective_max_tokens(&self) -> Option<i32> {
        self.max_completion_tokens.or(self.max_tokens)
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Message {
    pub role: String,
    pub content: MessageContent,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(untagged)]
pub enum MessageContent {
    Text(String),
    Parts(Vec<MessagePart>),
}

impl MessageContent {
    pub fn text_content(&self) -> String {
        match self {
            MessageContent::Text(s) => s.clone(),
            MessageContent::Parts(parts) => parts
                .iter()
                .filter_map(|p| {
                    if let MessagePart::Text { text } = p {
                        Some(text.as_str())
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
                .join(""),
        }
    }

    pub fn is_text_only(&self) -> bool {
        match self {
            MessageContent::Text(_) => true,
            MessageContent::Parts(parts) => parts.iter().all(|p| matches!(p, MessagePart::Text { .. })),
        }
    }
}

impl Default for MessageContent {
    fn default() -> Self {
        MessageContent::Text(String::new())
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "type")]
pub enum MessagePart {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image_url")]
    ImageUrl { image_url: ImageUrl },
    #[serde(rename = "input_audio")]
    InputAudio { input_audio: InputAudio },
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ImageUrl {
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct InputAudio {
    pub data: String,
    pub format: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ChatCompletionResponse {
    pub id: String,
    pub object: String,
    pub created: i64,
    pub model: String,
    pub choices: Vec<Choice>,
    pub usage: Option<Usage>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Choice {
    pub index: i32,
    pub message: Option<ResponseMessage>,
    pub delta: Option<Delta>,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ResponseMessage {
    pub role: Option<String>,
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Delta {
    pub role: Option<String>,
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Usage {
    pub prompt_tokens: i32,
    pub completion_tokens: i32,
    pub total_tokens: i32,
}

impl Usage {
    pub fn from_stream_chunks(chunks: &[Value]) -> Self {
        let mut prompt_tokens = 0;
        let mut completion_tokens = 0;
        let mut total_content = String::new();

        for chunk in chunks {
            if let Some(usage) = chunk.get("usage") {
                prompt_tokens = usage
                    .get("prompt_tokens")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0) as i32;
                completion_tokens = usage
                    .get("completion_tokens")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0) as i32;
            }

            if let Some(choices) = chunk.get("choices").and_then(|v| v.as_array()) {
                for choice in choices {
                    if let Some(delta) = choice.get("delta") {
                        if let Some(content) = delta.get("content").and_then(|v| v.as_str()) {
                            total_content.push_str(content);
                        }
                        if let Some(reasoning) = delta.get("reasoning_content").and_then(|v| v.as_str()) {
                            total_content.push_str(reasoning);
                        }
                    }
                }
            }
        }

        if prompt_tokens == 0 && completion_tokens == 0 && !total_content.is_empty() {
            completion_tokens = estimate_tokens_by_rules(&total_content).max(1) as i32;
            prompt_tokens = 10;
        }

        Self {
            prompt_tokens,
            completion_tokens,
            total_tokens: prompt_tokens + completion_tokens,
        }
    }

    pub fn from_ollama_response(response: &OllamaChatResponse) -> Self {
        Self {
            prompt_tokens: response.prompt_eval_count.unwrap_or(0) as i32,
            completion_tokens: response.eval_count.unwrap_or(0) as i32,
            total_tokens: (response.prompt_eval_count.unwrap_or(0)
                + response.eval_count.unwrap_or(0)) as i32,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimates_chinese_chars_as_one_token_each() {
        assert_eq!(estimate_tokens_by_rules("你好世界"), 4);
    }

    #[test]
    fn estimates_english_words_as_two_point_five_tokens_each() {
        assert_eq!(estimate_tokens_by_rules("hello world"), 5);
    }

    #[test]
    fn estimates_punctuation_as_one_token_each() {
        assert_eq!(estimate_tokens_by_rules("，。!?"), 4);
    }

    #[test]
    fn estimates_mixed_text_with_rules() {
        assert_eq!(estimate_tokens_by_rules("你好, hello world!"), 9);
    }

    #[test]
    fn anthropic_content_text_serializes_as_string() {
        let content = AnthropicContent::from_text("hello".to_string());
        let json = serde_json::to_string(&content).unwrap();
        assert_eq!(json, "\"hello\"");
    }

    #[test]
    fn anthropic_content_blocks_serializes_as_array() {
        let blocks = vec![
            AnthropicContentBlock::Image {
                source: AnthropicImageSource::Base64 {
                    media_type: "image/png".to_string(),
                    data: "iVBORw0KGgo=".to_string(),
                },
            },
            AnthropicContentBlock::Text {
                text: "What is this?".to_string(),
            },
        ];
        let content = AnthropicContent::from_blocks(blocks);
        let json = serde_json::to_string(&content).unwrap();
        assert!(json.contains("\"type\":\"image\""));
        assert!(json.contains("\"type\":\"base64\""));
        assert!(json.contains("\"media_type\":\"image/png\""));
        assert!(json.contains("\"type\":\"text\""));
        assert!(json.contains("\"text\":\"What is this?\""));
    }

    #[test]
    fn anthropic_image_url_source_serializes() {
        let source = AnthropicImageSource::Url {
            url: "https://example.com/img.png".to_string(),
        };
        let json = serde_json::to_string(&source).unwrap();
        assert!(json.contains("\"type\":\"url\""));
        assert!(json.contains("\"url\":\"https://example.com/img.png\""));
    }

    #[test]
    fn parse_data_uri_to_base64_source() {
        let source = parse_image_url_to_anthropic_source(
            "data:image/png;base64,iVBORw0KGgo=",
        );
        match source {
            AnthropicImageSource::Base64 { media_type, data } => {
                assert_eq!(media_type, "image/png");
                assert_eq!(data, "iVBORw0KGgo=");
            }
            _ => panic!("Expected Base64 source"),
        }
    }

    #[test]
    fn parse_http_url_to_url_source() {
        let source = parse_image_url_to_anthropic_source(
            "https://example.com/image.png",
        );
        match source {
            AnthropicImageSource::Url { url } => {
                assert_eq!(url, "https://example.com/image.png");
            }
            _ => panic!("Expected Url source"),
        }
    }

    #[test]
    fn convert_openai_image_url_to_anthropic() {
        let request = ChatCompletionRequest {
            model: "claude-3-sonnet".to_string(),
            messages: vec![
                Message {
                    role: "user".to_string(),
                    content: MessageContent::Parts(vec![
                        MessagePart::ImageUrl {
                            image_url: ImageUrl {
                                url: "data:image/jpeg;base64,/9j/4AAQ".to_string(),
                                detail: None,
                            },
                        },
                        MessagePart::Text {
                            text: "Describe this image".to_string(),
                        },
                    ]),
                    reasoning_content: None,
                },
            ],
            temperature: None,
            max_tokens: Some(1024),
            max_completion_tokens: None,
            stream: None,
        };

        let anthropic_req = convert_openai_to_anthropic_request(&request);
        assert_eq!(anthropic_req.messages.len(), 1);

        let msg = &anthropic_req.messages[0];
        assert_eq!(msg.role, "user");

        match &msg.content {
            AnthropicContent::Blocks(blocks) => {
                assert_eq!(blocks.len(), 2);
                assert!(matches!(&blocks[0], AnthropicContentBlock::Image { .. }));
                assert!(matches!(&blocks[1], AnthropicContentBlock::Text { .. }));
            }
            other => panic!("Expected Blocks, got: {:?}", other),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OllamaChatRequest {
    pub model: String,
    pub messages: Vec<OllamaMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options: Option<OllamaOptions>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OllamaOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub num_predict: Option<i32>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OllamaChatResponse {
    pub model: String,
    #[serde(default)]
    pub created_at: Option<String>,
    pub message: Option<OllamaMessage>,
    pub done: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub done_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_eval_count: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub eval_count: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_duration: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub load_duration: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_eval_duration: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub eval_duration: Option<i64>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct OllamaMessage {
    pub role: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OllamaStreamMessage {
    pub role: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OllamaStreamChunk {
    pub model: String,
    pub message: Option<OllamaStreamMessage>,
    pub done: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub done_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_eval_count: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub eval_count: Option<i64>,
}

pub fn is_ollama_api(upstream: &UpstreamConfig) -> bool {
    upstream.api_type == API_TYPE_OLLAMA
}

pub fn is_anthropic_api(upstream: &UpstreamConfig) -> bool {
    upstream.api_type == API_TYPE_ANTHROPIC || upstream.provider == "anthropic"
}

pub fn convert_openai_to_ollama_request(request: &ChatCompletionRequest) -> OllamaChatRequest {
    let messages: Vec<OllamaMessage> = request
        .messages
        .iter()
        .map(|m| OllamaMessage {
            role: m.role.clone(),
            content: m.content.text_content(),
            thinking: m.reasoning_content.clone(),
        })
        .collect();

    OllamaChatRequest {
        model: request.model.clone(),
        messages,
        stream: request.stream,
        options: Some(OllamaOptions {
            temperature: request.temperature,
            num_predict: request.effective_max_tokens(),
        }),
    }
}

pub fn convert_ollama_to_openai_response(
    ollama_response: OllamaChatResponse,
    model: &str,
) -> ChatCompletionResponse {
    let usage = Usage::from_ollama_response(&ollama_response);

    // 构建 Message，将 Ollama 的 thinking 映射到 reasoning_content
    let message = ollama_response.message.map(|m| ResponseMessage {
        role: Some(m.role),
        content: Some(m.content),
        reasoning_content: m.thinking.clone(),
    });

    ChatCompletionResponse {
        id: format!("chatcmpl-{}", uuid::Uuid::new_v4().simple()),
        object: "chat.completion".to_string(),
        created: chrono::Utc::now().timestamp(),
        model: model.to_string(),
        choices: vec![Choice {
            index: 0,
            message,
            delta: None,
            finish_reason: ollama_response.done_reason.clone(),
        }],
        usage: Some(usage),
    }
}

pub fn convert_ollama_stream_to_openai(
    chunk: OllamaStreamChunk,
    model: &str,
    chat_id: &str,
) -> Option<Value> {
    if chunk.done {
        let usage = Usage {
            prompt_tokens: chunk.prompt_eval_count.unwrap_or(0) as i32,
            completion_tokens: chunk.eval_count.unwrap_or(0) as i32,
            total_tokens: (chunk.prompt_eval_count.unwrap_or(0) + chunk.eval_count.unwrap_or(0))
                as i32,
        };

        Some(serde_json::json!({
            "id": chat_id,
            "object": "chat.completion.chunk",
            "created": chrono::Utc::now().timestamp(),
            "model": model,
            "choices": [{
                "index": 0,
                "delta": {"role": "assistant", "content": ""},
                "finish_reason": chunk.done_reason.unwrap_or_else(|| "stop".to_string())
            }],
            "usage": usage
        }))
    } else if let Some(message) = &chunk.message {
        // 构建 delta，包含 reasoning_content
        let mut delta = serde_json::json!({
            "role": "assistant",
            "content": message.content
        });

        // 如果 message 中有 thinking 内容，添加到 reasoning_content
        if let Some(ref thinking) = message.thinking {
            delta["reasoning_content"] = serde_json::json!(thinking);
        }

        Some(serde_json::json!({
            "id": chat_id,
            "object": "chat.completion.chunk",
            "created": chrono::Utc::now().timestamp(),
            "model": model,
            "choices": [{
                "index": 0,
                "delta": delta,
                "finish_reason": null
            }]
        }))
    } else {
        None
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AnthropicMessageRequest {
    pub model: String,
    pub messages: Vec<AnthropicMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(untagged)]
pub enum AnthropicContent {
    Text(String),
    Blocks(Vec<AnthropicContentBlock>),
}

impl AnthropicContent {
    pub fn text_only(&self) -> String {
        match self {
            AnthropicContent::Text(s) => s.clone(),
            AnthropicContent::Blocks(blocks) => blocks
                .iter()
                .filter_map(|b| {
                    if let AnthropicContentBlock::Text { text } = b {
                        Some(text.as_str())
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
                .join(""),
        }
    }

    pub fn from_text(s: String) -> Self {
        AnthropicContent::Text(s)
    }

    pub fn from_blocks(blocks: Vec<AnthropicContentBlock>) -> Self {
        if blocks.len() == 1 {
            if let AnthropicContentBlock::Text { ref text } = blocks[0] {
                return AnthropicContent::Text(text.clone());
            }
        }
        AnthropicContent::Blocks(blocks)
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "type")]
pub enum AnthropicContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image")]
    Image { source: AnthropicImageSource },
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "type")]
pub enum AnthropicImageSource {
    #[serde(rename = "base64")]
    Base64 {
        media_type: String,
        data: String,
    },
    #[serde(rename = "url")]
    Url { url: String },
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AnthropicMessage {
    pub role: String,
    pub content: AnthropicContent,
}

pub fn convert_openai_to_anthropic_request(
    request: &ChatCompletionRequest,
) -> AnthropicMessageRequest {
    let mut system_parts: Vec<String> = Vec::new();
    let mut messages = Vec::new();

    for msg in &request.messages {
        if msg.role == "system" {
            system_parts.push(msg.content.text_content());
        } else {
            let content = convert_openai_content_to_anthropic(&msg.content);
            messages.push(AnthropicMessage {
                role: msg.role.clone(),
                content,
            });
        }
    }

    let system = if system_parts.is_empty() {
        None
    } else {
        Some(system_parts.join("\n\n"))
    };

    let messages = merge_consecutive_roles(messages);

    AnthropicMessageRequest {
        model: request.model.clone(),
        messages,
        max_tokens: request.effective_max_tokens().or(Some(8192)),
        temperature: request.temperature,
        stream: request.stream,
        system,
    }
}

fn convert_openai_content_to_anthropic(content: &MessageContent) -> AnthropicContent {
    match content {
        MessageContent::Text(s) => AnthropicContent::from_text(s.clone()),
        MessageContent::Parts(parts) => {
            let mut blocks: Vec<AnthropicContentBlock> = Vec::new();
            for part in parts {
                match part {
                    MessagePart::Text { text } => {
                        blocks.push(AnthropicContentBlock::Text { text: text.clone() });
                    }
                    MessagePart::ImageUrl { image_url } => {
                        let source = parse_image_url_to_anthropic_source(&image_url.url);
                        blocks.push(AnthropicContentBlock::Image { source });
                    }
                    MessagePart::InputAudio { .. } => {}
                }
            }
            if blocks.is_empty() {
                AnthropicContent::from_text(String::new())
            } else {
                AnthropicContent::from_blocks(blocks)
            }
        }
    }
}

fn parse_image_url_to_anthropic_source(url: &str) -> AnthropicImageSource {
    if let Some(rest) = url.strip_prefix("data:") {
        if let Some(semicolon_pos) = rest.find(';') {
            let media_type = rest[..semicolon_pos].to_string();
            let after_semicolon = &rest[semicolon_pos + 1..];
            if let Some(data) = after_semicolon.strip_prefix("base64,") {
                return AnthropicImageSource::Base64 {
                    media_type,
                    data: data.to_string(),
                };
            }
        }
        AnthropicImageSource::Url { url: url.to_string() }
    } else {
        AnthropicImageSource::Url { url: url.to_string() }
    }
}

fn merge_consecutive_roles(messages: Vec<AnthropicMessage>) -> Vec<AnthropicMessage> {
    if messages.is_empty() {
        return messages;
    }
    let mut result: Vec<AnthropicMessage> = Vec::with_capacity(messages.len());
    result.push(messages[0].clone());
    for msg in messages.iter().skip(1) {
        if let Some(last) = result.last_mut() {
            if last.role == msg.role {
                let last_text = last.content.text_only();
                let msg_text = msg.content.text_only();
                let merged = format!("{}\n\n{}", last_text, msg_text);
                last.content = AnthropicContent::from_text(merged);
                continue;
            }
        }
        result.push(msg.clone());
    }
    if result.first().map(|m| m.role.as_str()) == Some("assistant") {
        result.remove(0);
    }
    result
}

pub fn convert_anthropic_to_openai_response(
    response: &serde_json::Value,
    model: &str,
) -> ChatCompletionResponse {
    let mut content = String::new();
    let mut reasoning_content = String::new();
    if let Some(content_blocks) = response.get("content").and_then(|c| c.as_array()) {
        for block in content_blocks {
            let block_type = block.get("type").and_then(|t| t.as_str()).unwrap_or("");
            match block_type {
                "text" => {
                    if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                        content.push_str(text);
                    }
                }
                "thinking" => {
                    if let Some(thinking) = block.get("thinking").and_then(|t| t.as_str()) {
                        reasoning_content.push_str(thinking);
                    }
                }
                _ => {}
            }
        }
    }

    let input_tokens = response
        .get("usage")
        .and_then(|u| u.get("input_tokens"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0) as i32;
    let output_tokens = response
        .get("usage")
        .and_then(|u| u.get("output_tokens"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0) as i32;

    let stop_reason = response
        .get("stop_reason")
        .and_then(|v| v.as_str())
        .unwrap_or("stop");

    let finish_reason = match stop_reason {
        "end_turn" | "stop" => "stop".to_string(),
        "max_tokens" => "length".to_string(),
        other => other.to_string(),
    };

    ChatCompletionResponse {
        id: format!("chatcmpl-{}", uuid::Uuid::new_v4().simple()),
        object: "chat.completion".to_string(),
        created: chrono::Utc::now().timestamp(),
        model: model.to_string(),
        choices: vec![Choice {
            index: 0,
            message: Some(ResponseMessage {
                role: Some("assistant".to_string()),
                content: if content.is_empty() {
                    None
                } else {
                    Some(content)
                },
                reasoning_content: if reasoning_content.is_empty() {
                    None
                } else {
                    Some(reasoning_content)
                },
            }),
            delta: None,
            finish_reason: Some(finish_reason),
        }],
        usage: Some(Usage {
            prompt_tokens: input_tokens,
            completion_tokens: output_tokens,
            total_tokens: input_tokens + output_tokens,
        }),
    }
}

pub fn convert_anthropic_stream_to_openai(
    event_name: &str,
    data: &serde_json::Value,
    model: &str,
    chat_id: &str,
) -> Vec<Option<Value>> {
    let mut chunks = Vec::new();

    match event_name {
        "message_start" => {
            chunks.push(Some(serde_json::json!({
                "id": chat_id,
                "object": "chat.completion.chunk",
                "created": chrono::Utc::now().timestamp(),
                "model": model,
                "choices": [{
                    "index": 0,
                    "delta": {"role": "assistant", "content": ""},
                    "finish_reason": null
                }]
            })));
        }
        "content_block_start" => {
            let block_type = data
                .get("content_block")
                .and_then(|cb| cb.get("type"))
                .and_then(|t| t.as_str())
                .unwrap_or("");
            if block_type == "thinking" {
                if let Some(thinking) = data
                    .get("content_block")
                    .and_then(|cb| cb.get("thinking"))
                    .and_then(|t| t.as_str())
                {
                    if !thinking.is_empty() {
                        chunks.push(Some(serde_json::json!({
                            "id": chat_id,
                            "object": "chat.completion.chunk",
                            "created": chrono::Utc::now().timestamp(),
                            "model": model,
                            "choices": [{
                                "index": 0,
                                "delta": {"reasoning_content": thinking},
                                "finish_reason": null
                            }]
                        })));
                    }
                }
            } else if let Some(text) = data
                .get("content_block")
                .and_then(|cb| cb.get("text"))
                .and_then(|t| t.as_str())
            {
                if !text.is_empty() {
                    chunks.push(Some(serde_json::json!({
                        "id": chat_id,
                        "object": "chat.completion.chunk",
                        "created": chrono::Utc::now().timestamp(),
                        "model": model,
                        "choices": [{
                            "index": 0,
                            "delta": {"content": text},
                            "finish_reason": null
                        }]
                    })));
                }
            }
        }
        "content_block_delta" => {
            let delta_type = data
                .get("delta")
                .and_then(|d| d.get("type"))
                .and_then(|t| t.as_str())
                .unwrap_or("");
            if delta_type == "thinking_delta" {
                if let Some(thinking) = data
                    .get("delta")
                    .and_then(|d| d.get("thinking"))
                    .and_then(|t| t.as_str())
                {
                    chunks.push(Some(serde_json::json!({
                        "id": chat_id,
                        "object": "chat.completion.chunk",
                        "created": chrono::Utc::now().timestamp(),
                        "model": model,
                        "choices": [{
                            "index": 0,
                            "delta": {"reasoning_content": thinking},
                            "finish_reason": null
                        }]
                    })));
                }
            } else if let Some(text) = data
                .get("delta")
                .and_then(|d| d.get("text"))
                .and_then(|t| t.as_str())
            {
                chunks.push(Some(serde_json::json!({
                    "id": chat_id,
                    "object": "chat.completion.chunk",
                    "created": chrono::Utc::now().timestamp(),
                    "model": model,
                    "choices": [{
                        "index": 0,
                        "delta": {"content": text},
                        "finish_reason": null
                    }]
                })));
            }
        }
        "message_delta" => {
            let stop_reason = data
                .get("delta")
                .and_then(|d| d.get("stop_reason"))
                .and_then(|v| v.as_str())
                .unwrap_or("stop");
            let finish_reason = match stop_reason {
                "end_turn" | "stop" => "stop",
                "max_tokens" => "length",
                other => other,
            };

            let mut usage_json = serde_json::json!({});
            if let Some(usage) = data.get("usage") {
                if let Some(ot) = usage.get("output_tokens").and_then(|v| v.as_i64()) {
                    usage_json = serde_json::json!({
                        "prompt_tokens": 0,
                        "completion_tokens": ot,
                        "total_tokens": ot
                    });
                }
            }

            chunks.push(Some(serde_json::json!({
                "id": chat_id,
                "object": "chat.completion.chunk",
                "created": chrono::Utc::now().timestamp(),
                "model": model,
                "choices": [{
                    "index": 0,
                    "delta": {},
                    "finish_reason": finish_reason
                }],
                "usage": usage_json
            })));
        }
        "message_stop" => {
            chunks.push(Some(serde_json::json!({
                "id": chat_id,
                "object": "chat.completion.chunk",
                "created": chrono::Utc::now().timestamp(),
                "model": model,
                "choices": [{
                    "index": 0,
                    "delta": {},
                    "finish_reason": "stop"
                }]
            })));
        }
        _ => {}
    }

    chunks
}
