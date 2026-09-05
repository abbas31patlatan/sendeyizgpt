//! Provider adapters for local-first and OpenAI-compatible chat backends.
//!
//! The adapter deliberately owns HTTP details, SSE framing, response limits and
//! cancellation. The desktop shell only receives typed chat events and never
//! needs to handle provider-specific wire formats.

pub mod tools;

pub use tools::{
    BuiltinToolError, ToolExecution, builtin_tool_definitions, execute_builtin_tool,
    tool_system_instructions,
};

use futures_util::StreamExt;
use reqwest::{Response, header};
use serde::{Deserialize, Serialize};
use std::fmt::{Debug, Formatter};
use std::time::{Duration, Instant};
use thiserror::Error;
use tokio_util::sync::CancellationToken;
use url::{Host, Url};

const MAX_MESSAGES: usize = 128;
const MAX_MESSAGE_BYTES: usize = 256 * 1024;
const MAX_SSE_BUFFER_BYTES: usize = 4 * 1024 * 1024;
const MAX_TOOL_DEFINITIONS: usize = 32;
const MAX_JSON_RESPONSE_BYTES: usize = 8 * 1024 * 1024;

fn default_base_url() -> String {
    "http://127.0.0.1:11434/v1".to_owned()
}

fn default_max_tokens() -> u32 {
    1024
}

fn default_temperature() -> f32 {
    0.7
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderConfig {
    #[serde(default = "default_base_url")]
    pub base_url: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
}

impl Debug for ProviderConfig {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProviderConfig")
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .field("api_key", &self.api_key.as_ref().map(|_| "[REDACTED]"))
            .finish()
    }
}

impl ProviderConfig {
    pub fn validate(&self) -> Result<Url, ProviderError> {
        let url = Url::parse(self.base_url.trim()).map_err(|error| {
            ProviderError::InvalidConfig(format!("base URL is invalid: {error}"))
        })?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(ProviderError::InvalidConfig(
                "base URL must use http or https".to_owned(),
            ));
        }
        if url.scheme() == "http" && !is_loopback_host(&url) {
            return Err(ProviderError::InvalidConfig(
                "plain HTTP is allowed only for a loopback local provider".to_owned(),
            ));
        }
        if url.host_str().is_none() {
            return Err(ProviderError::InvalidConfig(
                "base URL must include a host".to_owned(),
            ));
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(ProviderError::InvalidConfig(
                "credentials in the base URL are not allowed".to_owned(),
            ));
        }
        if url.query().is_some() || url.fragment().is_some() {
            return Err(ProviderError::InvalidConfig(
                "query strings and fragments are not allowed in the base URL".to_owned(),
            ));
        }
        if self.model.trim().is_empty() {
            return Err(ProviderError::InvalidConfig(
                "a model name is required".to_owned(),
            ));
        }
        if self.model.len() > 256 {
            return Err(ProviderError::InvalidConfig(
                "model name is too long".to_owned(),
            ));
        }
        Ok(url)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChatRequest {
    pub provider: ProviderConfig,
    pub messages: Vec<ChatMessage>,
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
    #[serde(default = "default_temperature")]
    pub temperature: f32,
    #[serde(default)]
    pub tools: Vec<ToolDefinition>,
    #[serde(default)]
    pub worker: Option<ProviderConfig>,
    #[serde(default)]
    pub web_tools: bool,
}

impl ChatRequest {
    pub fn validate(&self) -> Result<(), ProviderError> {
        self.provider.validate()?;
        if self.messages.is_empty() {
            return Err(ProviderError::InvalidRequest(
                "at least one chat message is required".to_owned(),
            ));
        }
        if self.messages.len() > MAX_MESSAGES {
            return Err(ProviderError::InvalidRequest(format!(
                "chat history cannot contain more than {MAX_MESSAGES} messages"
            )));
        }
        if self.tools.len() > MAX_TOOL_DEFINITIONS {
            return Err(ProviderError::InvalidRequest(format!(
                "a chat request cannot expose more than {MAX_TOOL_DEFINITIONS} tools"
            )));
        }
        if let Some(worker) = &self.worker {
            worker.validate()?;
        }
        if self.max_tokens == 0 || self.max_tokens > 131_072 {
            return Err(ProviderError::InvalidRequest(
                "max_tokens must be between 1 and 131072".to_owned(),
            ));
        }
        if !self.temperature.is_finite() || !(0.0..=2.0).contains(&self.temperature) {
            return Err(ProviderError::InvalidRequest(
                "temperature must be a finite value between 0 and 2".to_owned(),
            ));
        }
        for message in &self.messages {
            let tool_call_message =
                message.role == ChatRole::Assistant && message.tool_calls.is_some();
            if message.content.trim().is_empty() && !tool_call_message {
                return Err(ProviderError::InvalidRequest(
                    "chat messages cannot be empty".to_owned(),
                ));
            }
            if message.content.len() > MAX_MESSAGE_BYTES {
                return Err(ProviderError::InvalidRequest(format!(
                    "a chat message cannot exceed {MAX_MESSAGE_BYTES} bytes"
                )));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<AssistantToolCall>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AssistantToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub function: ToolCallFunction,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolCallFunction {
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolDefinition {
    #[serde(rename = "type")]
    pub kind: String,
    pub function: ToolFunctionDefinition,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolFunctionDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

impl ToolDefinition {
    pub fn function(
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: serde_json::Value,
    ) -> Self {
        Self {
            kind: "function".to_owned(),
            function: ToolFunctionDefinition {
                name: name.into(),
                description: description.into(),
                parameters,
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolCallDelta {
    pub index: usize,
    pub id: Option<String>,
    pub name: Option<String>,
    pub arguments: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ChatRole {
    System,
    Developer,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChatChunk {
    Content { text: String },
    Reasoning { text: String },
    ToolCallDelta(ToolCallDelta),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderModel {
    pub id: String,
    pub owned_by: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChatCompletionSummary {
    pub generated_tokens: u64,
    pub prompt_tokens: Option<u64>,
    pub time_to_first_token_ms: Option<f64>,
    pub generation_duration_ms: f64,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChatCompletion {
    pub message: ChatMessage,
    pub prompt_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
    pub finish_reason: Option<String>,
}

#[derive(Clone)]
pub struct OpenAiCompatibleClient {
    http: reqwest::Client,
    config: ProviderConfig,
    base_url: Url,
}

impl OpenAiCompatibleClient {
    pub fn new(config: ProviderConfig) -> Result<Self, ProviderError> {
        let base_url = config.validate()?;
        let http = reqwest::Client::builder()
            .user_agent("Aegis-AI/0.2")
            .connect_timeout(Duration::from_secs(5))
            .read_timeout(Duration::from_secs(120))
            .build()
            .map_err(|error| ProviderError::Client(error.to_string()))?;
        Ok(Self {
            http,
            config,
            base_url,
        })
    }

    pub async fn list_models(&self) -> Result<Vec<ProviderModel>, ProviderError> {
        let response = self
            .authorized(self.http.get(self.endpoint("models")))
            .send()
            .await
            .map_err(|error| ProviderError::Transport(error.to_string()))?;
        let response = checked_response(response).await?;
        let payload: ModelsResponse = response
            .json()
            .await
            .map_err(|error| ProviderError::InvalidResponse(error.to_string()))?;
        Ok(payload
            .data
            .into_iter()
            .filter(|model| !model.id.trim().is_empty())
            .map(|model| ProviderModel {
                id: model.id,
                owned_by: model.owned_by,
            })
            .collect())
    }

    pub fn model_name(&self) -> &str {
        &self.config.model
    }

    pub fn config_clone(&self) -> ProviderConfig {
        self.config.clone()
    }

    pub async fn stream_chat<F>(
        &self,
        request: ChatRequest,
        cancellation: CancellationToken,
        mut on_chunk: F,
    ) -> Result<ChatCompletionSummary, ProviderError>
    where
        F: FnMut(ChatChunk) + Send,
    {
        request.validate()?;
        let started = Instant::now();
        let body = ApiChatRequest {
            model: request.provider.model.clone(),
            messages: request.messages,
            max_tokens: request.max_tokens,
            temperature: request.temperature,
            stream: true,
            tools: request.tools,
        };
        let response = tokio::select! {
            _ = cancellation.cancelled() => return Err(ProviderError::Cancelled),
            result = self.authorized(self.http.post(self.endpoint("chat/completions")).json(&body)).send() => {
                result.map_err(|error| ProviderError::Transport(error.to_string()))?
            }
        };
        let response = checked_response(response).await?;
        let mut stream = response.bytes_stream();
        let mut parser = SseParser::default();
        let mut generated_tokens = 0_u64;
        let mut prompt_tokens = None;
        let mut time_to_first_token_ms = None;
        let mut finish_reason = None;
        let mut stream_finished = false;

        while let Some(next) = tokio::select! {
            _ = cancellation.cancelled() => return Err(ProviderError::Cancelled),
            next = stream.next() => next,
        } {
            let bytes = next.map_err(|error| ProviderError::Transport(error.to_string()))?;
            let text = std::str::from_utf8(&bytes)
                .map_err(|error| ProviderError::InvalidResponse(error.to_string()))?;
            for event in parser.push(text)? {
                if process_event(
                    event,
                    &mut generated_tokens,
                    &mut prompt_tokens,
                    &mut time_to_first_token_ms,
                    &mut finish_reason,
                    &mut on_chunk,
                    started,
                )? {
                    stream_finished = true;
                    break;
                }
            }
            if stream_finished {
                break;
            }
        }

        if !stream_finished {
            for event in parser.finish()? {
                if process_event(
                    event,
                    &mut generated_tokens,
                    &mut prompt_tokens,
                    &mut time_to_first_token_ms,
                    &mut finish_reason,
                    &mut on_chunk,
                    started,
                )? {
                    break;
                }
            }
        }

        Ok(ChatCompletionSummary {
            generated_tokens,
            prompt_tokens,
            time_to_first_token_ms,
            generation_duration_ms: started.elapsed().as_secs_f64() * 1000.0,
            finish_reason,
        })
    }

    /// Performs one bounded non-streaming completion. The worker path uses
    /// this for delegated subtasks so the master stream remains cancellable.
    pub async fn complete_chat(
        &self,
        request: ChatRequest,
        cancellation: CancellationToken,
    ) -> Result<ChatCompletion, ProviderError> {
        request.validate()?;
        let body = ApiChatRequest {
            model: request.provider.model.clone(),
            messages: request.messages,
            max_tokens: request.max_tokens,
            temperature: request.temperature,
            stream: false,
            tools: request.tools,
        };
        let response = tokio::select! {
            _ = cancellation.cancelled() => return Err(ProviderError::Cancelled),
            result = self.authorized(self.http.post(self.endpoint("chat/completions")).json(&body)).send() => {
                result.map_err(|error| ProviderError::Transport(error.to_string()))?
            }
        };
        let response = checked_response(response).await?;
        let bytes = bounded_response_bytes(
            response,
            &cancellation,
            MAX_JSON_RESPONSE_BYTES,
        )
        .await?;
        let payload: ApiChatResponse = serde_json::from_slice(&bytes)
            .map_err(|error| ProviderError::InvalidResponse(error.to_string()))?;
        let choice = payload
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| ProviderError::InvalidResponse("provider returned no choices".to_owned()))?;
        let message = choice.message;
        Ok(ChatCompletion {
            message: ChatMessage {
                role: message.role,
                content: message.content.unwrap_or_default(),
                name: None,
                tool_call_id: None,
                tool_calls: message.tool_calls,
            },
            prompt_tokens: payload.usage.as_ref().and_then(|usage| usage.prompt_tokens),
            completion_tokens: payload
                .usage
                .as_ref()
                .and_then(|usage| usage.completion_tokens),
            finish_reason: choice.finish_reason,
        })
    }

    fn endpoint(&self, path: &str) -> Url {
        let mut url = self.base_url.clone();
        let mut base_path = url.path().trim_end_matches('/').to_owned();
        base_path.push('/');
        base_path.push_str(path.trim_start_matches('/'));
        url.set_path(&base_path);
        url
    }

    fn authorized(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match self.config.api_key.as_deref().map(str::trim) {
            Some(key) if !key.is_empty() => {
                request.header(header::AUTHORIZATION, format!("Bearer {key}"))
            }
            _ => request,
        }
    }
}

fn is_loopback_host(url: &Url) -> bool {
    match url.host() {
        Some(Host::Domain(host)) => host.eq_ignore_ascii_case("localhost"),
        Some(Host::Ipv4(address)) => address.is_loopback(),
        Some(Host::Ipv6(address)) => address.is_loopback(),
        None => false,
    }
}

async fn checked_response(response: Response) -> Result<Response, ProviderError> {
    if response.status().is_success() {
        return Ok(response);
    }
    let status = response.status().as_u16();
    let body = response.text().await.unwrap_or_default();
    Err(ProviderError::HttpStatus {
        status,
        body: sanitize_error(&body),
    })
}

async fn bounded_response_bytes(
    response: Response,
    cancellation: &CancellationToken,
    limit: usize,
) -> Result<Vec<u8>, ProviderError> {
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err(ProviderError::InvalidResponse(
            "provider response exceeded the size limit".to_owned(),
        ));
    }
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(next) = tokio::select! {
        _ = cancellation.cancelled() => return Err(ProviderError::Cancelled),
        next = stream.next() => next,
    } {
        let chunk = next.map_err(|error| ProviderError::Transport(error.to_string()))?;
        if bytes.len().saturating_add(chunk.len()) > limit {
            return Err(ProviderError::InvalidResponse(
                "provider response exceeded the size limit".to_owned(),
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn sanitize_error(value: &str) -> String {
    let compact = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.is_empty() {
        return "provider returned no additional details".to_owned();
    }
    compact.chars().take(512).collect()
}

fn process_event<F>(
    event: SseEvent,
    generated_tokens: &mut u64,
    prompt_tokens: &mut Option<u64>,
    time_to_first_token_ms: &mut Option<f64>,
    finish_reason: &mut Option<String>,
    on_chunk: &mut F,
    started: Instant,
) -> Result<bool, ProviderError>
where
    F: FnMut(ChatChunk),
{
    if event.data.trim() == "[DONE]" {
        return Ok(true);
    }
    let payload: ApiStreamChunk = serde_json::from_str(event.data.trim())
        .map_err(|error| ProviderError::InvalidResponse(error.to_string()))?;
    if let Some(usage) = payload.usage {
        *prompt_tokens = usage.prompt_tokens;
        if let Some(completion_tokens) = usage.completion_tokens {
            *generated_tokens = completion_tokens;
        }
    }
    let Some(choice) = payload.choices.first() else {
        return Ok(false);
    };
    if choice.finish_reason.is_some() {
        *finish_reason = choice.finish_reason.clone();
    }
    let Some(delta) = choice.delta.as_ref() else {
        return Ok(false);
    };
    for tool_call in &delta.tool_calls {
        on_chunk(ChatChunk::ToolCallDelta(ToolCallDelta {
            index: tool_call.index.unwrap_or_default(),
            id: tool_call.id.clone(),
            name: tool_call.function.as_ref().and_then(|function| function.name.clone()),
            arguments: tool_call
                .function
                .as_ref()
                .and_then(|function| function.arguments.clone()),
        }));
    }
    if let Some(text) = delta
        .reasoning_content
        .as_deref()
        .filter(|text| !text.is_empty())
    {
        on_chunk(ChatChunk::Reasoning {
            text: text.to_owned(),
        });
    }
    if let Some(text) = delta.content.as_deref().filter(|text| !text.is_empty()) {
        if time_to_first_token_ms.is_none() {
            *time_to_first_token_ms = Some(started.elapsed().as_secs_f64() * 1000.0);
        }
        *generated_tokens = generated_tokens.saturating_add(estimate_tokens(text));
        on_chunk(ChatChunk::Content {
            text: text.to_owned(),
        });
    }
    Ok(false)
}

fn estimate_tokens(text: &str) -> u64 {
    ((text.chars().count() as u64).saturating_add(3)) / 4
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseEvent {
    pub data: String,
}

#[derive(Debug, Default)]
pub struct SseParser {
    buffer: String,
}

impl SseParser {
    pub fn push(&mut self, chunk: &str) -> Result<Vec<SseEvent>, ProviderError> {
        self.buffer.push_str(chunk);
        self.buffer = self.buffer.replace("\r\n", "\n").replace('\r', "\n");
        if self.buffer.len() > MAX_SSE_BUFFER_BYTES {
            return Err(ProviderError::InvalidResponse(
                "provider SSE frame exceeded the size limit".to_owned(),
            ));
        }
        Ok(self.take_complete_events())
    }

    pub fn finish(&mut self) -> Result<Vec<SseEvent>, ProviderError> {
        if self.buffer.trim().is_empty() {
            self.buffer.clear();
            return Ok(Vec::new());
        }
        let frame = std::mem::take(&mut self.buffer);
        Ok(parse_sse_frame(&frame).into_iter().collect())
    }

    fn take_complete_events(&mut self) -> Vec<SseEvent> {
        let mut events = Vec::new();
        while let Some(position) = self.buffer.find("\n\n") {
            let frame = self.buffer[..position].to_owned();
            self.buffer.drain(..position + 2);
            if let Some(event) = parse_sse_frame(&frame) {
                events.push(event);
            }
        }
        events
    }
}

fn parse_sse_frame(frame: &str) -> Option<SseEvent> {
    let data = frame
        .lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .map(|line| line.strip_prefix(' ').unwrap_or(line))
        .collect::<Vec<_>>();
    if data.is_empty() {
        None
    } else {
        Some(SseEvent {
            data: data.join("\n"),
        })
    }
}

#[derive(Debug, Deserialize)]
struct ModelsResponse {
    #[serde(default)]
    data: Vec<ModelResponse>,
}

#[derive(Debug, Deserialize)]
struct ModelResponse {
    id: String,
    #[serde(default)]
    owned_by: Option<String>,
}

#[derive(Debug, Serialize)]
struct ApiChatRequest {
    model: String,
    messages: Vec<ChatMessage>,
    max_tokens: u32,
    temperature: f32,
    stream: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<ToolDefinition>,
}

#[derive(Debug, Deserialize)]
struct ApiStreamChunk {
    #[serde(default)]
    choices: Vec<ApiChoice>,
    #[serde(default)]
    usage: Option<ApiUsage>,
}

#[derive(Debug, Deserialize)]
struct ApiChoice {
    #[serde(default)]
    delta: Option<ApiDelta>,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ApiDelta {
    #[serde(default)]
    content: Option<String>,
    #[serde(default, alias = "reasoning")]
    reasoning_content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<ApiToolCallDelta>,
}

#[derive(Debug, Deserialize)]
struct ApiToolCallDelta {
    #[serde(default)]
    index: Option<usize>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: Option<ApiFunctionDelta>,
}

#[derive(Debug, Deserialize)]
struct ApiFunctionDelta {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ApiChatResponse {
    #[serde(default)]
    choices: Vec<ApiCompletionChoice>,
    #[serde(default)]
    usage: Option<ApiUsage>,
}

#[derive(Debug, Deserialize)]
struct ApiCompletionChoice {
    message: ApiResponseMessage,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ApiResponseMessage {
    role: ChatRole,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<AssistantToolCall>>,
}

#[derive(Debug, Deserialize)]
struct ApiUsage {
    #[serde(default)]
    prompt_tokens: Option<u64>,
    #[serde(default)]
    completion_tokens: Option<u64>,
}

#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("provider configuration is invalid: {0}")]
    InvalidConfig(String),
    #[error("chat request is invalid: {0}")]
    InvalidRequest(String),
    #[error("provider HTTP request failed: {0}")]
    Transport(String),
    #[error("provider client could not start: {0}")]
    Client(String),
    #[error("provider returned HTTP {status}: {body}")]
    HttpStatus { status: u16, body: String },
    #[error("provider returned an invalid response: {0}")]
    InvalidResponse(String),
    #[error("provider request was cancelled")]
    Cancelled,
}

impl ProviderError {
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::Transport(_) => true,
            Self::HttpStatus { status, .. } => {
                *status == 408 || *status == 409 || *status == 429 || *status >= 500
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> ProviderConfig {
        ProviderConfig {
            base_url: "http://127.0.0.1:11434/v1/".to_owned(),
            model: "llama3.2".to_owned(),
            api_key: None,
        }
    }

    #[test]
    fn validates_local_provider_and_builds_expected_endpoint() {
        let client = OpenAiCompatibleClient::new(config()).expect("client creates");
        assert_eq!(
            client.endpoint("chat/completions").as_str(),
            "http://127.0.0.1:11434/v1/chat/completions"
        );
    }

    #[test]
    fn rejects_credentials_in_url_and_empty_messages() {
        let mut invalid = config();
        invalid.base_url = "https://user:secret@example.test/v1".to_owned();
        assert!(matches!(
            invalid.validate(),
            Err(ProviderError::InvalidConfig(_))
        ));

        invalid.base_url = "http://example.test/v1".to_owned();
        assert!(matches!(
            invalid.validate(),
            Err(ProviderError::InvalidConfig(_))
        ));

        let request = ChatRequest {
            provider: config(),
            messages: Vec::new(),
            max_tokens: 10,
            temperature: 0.7,
            tools: Vec::new(),
            worker: None,
            web_tools: false,
        };
        assert!(matches!(
            request.validate(),
            Err(ProviderError::InvalidRequest(_))
        ));
    }

    #[test]
    fn parses_chunked_sse_and_joins_multiline_data() {
        let mut parser = SseParser::default();
        assert!(
            parser
                .push("data: {\"choices\":[{\"delta\":{\"content\":\"he")
                .expect("first")
                .is_empty()
        );
        let events = parser
            .push("llo\"}}]}\n\ndata: [DONE]\n\n")
            .expect("second");
        assert_eq!(events.len(), 2);
        assert!(events[0].data.contains("hello"));
        assert_eq!(events[1].data, "[DONE]");
    }

    #[test]
    fn redacts_provider_key_in_debug_output() {
        let secret = "super-secret-key";
        let config = ProviderConfig {
            api_key: Some(secret.to_owned()),
            ..config()
        };
        let rendered = format!("{config:?}");
        assert!(!rendered.contains(secret));
        assert!(rendered.contains("REDACTED"));
    }

    #[test]
    fn retryability_is_limited_to_transient_failures() {
        assert!(
            ProviderError::HttpStatus {
                status: 429,
                body: "busy".to_owned()
            }
            .is_retryable()
        );
        assert!(!ProviderError::InvalidRequest("bad".to_owned()).is_retryable());
    }

    #[test]
    fn serializes_openai_tool_call_round_trip_messages() {
        let tool = ToolDefinition::function(
            "calculator",
            "Evaluate arithmetic",
            serde_json::json!({
                "type": "object",
                "properties": {"expression": {"type": "string"}}
            }),
        );
        let body = ApiChatRequest {
            model: "master".to_owned(),
            messages: vec![
                ChatMessage {
                    role: ChatRole::Assistant,
                    content: String::new(),
                    name: None,
                    tool_call_id: None,
                    tool_calls: Some(vec![AssistantToolCall {
                        id: "call-1".to_owned(),
                        kind: "function".to_owned(),
                        function: ToolCallFunction {
                            name: "calculator".to_owned(),
                            arguments: r#"{"expression":"2+2"}"#.to_owned(),
                        },
                    }]),
                },
                ChatMessage {
                    role: ChatRole::Tool,
                    content: r#"{"result":4}"#.to_owned(),
                    name: Some("calculator".to_owned()),
                    tool_call_id: Some("call-1".to_owned()),
                    tool_calls: None,
                },
            ],
            max_tokens: 64,
            temperature: 0.2,
            stream: true,
            tools: vec![tool],
        };
        let value = serde_json::to_value(body).expect("tool request serializes");
        assert_eq!(value["tools"][0]["type"], "function");
        assert_eq!(value["messages"][0]["tool_calls"][0]["id"], "call-1");
        assert_eq!(value["messages"][1]["tool_call_id"], "call-1");
    }
}
