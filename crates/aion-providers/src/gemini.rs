use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderValue, RETRY_AFTER, USER_AGENT};
use serde_json::{Value, json};
use tokio::sync::mpsc;

use aion_config::compat::{self, ProviderCompat};
use aion_types::llm::{LlmEvent, LlmRequest, ThinkingConfig};
use aion_types::message::{ContentBlock, Message, Role, StopReason, TokenUsage};
use aion_types::tool::{ToolDef, truncate_deferred_description};

use crate::anthropic_shared::StreamOutcome;
use crate::retry::with_retry_if_notify_budget;
use crate::{LlmProvider, ProviderError, RetryObserver};

static TOOL_CALL_COUNTER: AtomicU64 = AtomicU64::new(1);

const GEMINI_USER_AGENT: &str = concat!("aionrs/", env!("CARGO_PKG_VERSION"));
const STREAM_REQUEST_CONNECTION_RETRIES: u32 = 4;
const STREAM_REQUEST_RATE_LIMIT_RETRIES: u32 = 12;
const STREAM_IDLE_TIMEOUT: Duration = Duration::from_millis(300_000);
const SYNTHETIC_THOUGHT_SIGNATURE: &str = "skip_thought_signature_validator";
const GOOGLE_GEMINI_ROOT_URL: &str = "https://generativelanguage.googleapis.com";
const GOOGLE_GEMINI_DEFAULT_BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta";

pub struct GeminiProvider {
    client: reqwest::Client,
    api_key: String,
    base_url: String,
    compat: ProviderCompat,
}

impl GeminiProvider {
    pub fn new(api_key: &str, base_url: &str, compat: ProviderCompat) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_key: api_key.to_string(),
            base_url: normalize_base_url(base_url),
            compat,
        }
    }

    fn build_headers(&self) -> Result<HeaderMap, ProviderError> {
        let mut headers = HeaderMap::new();
        let api_key = HeaderValue::from_str(&self.api_key).map_err(|e| {
            ProviderError::Connection(format!("Invalid x-goog-api-key header: {e}"))
        })?;
        headers.insert("x-goog-api-key", api_key);
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(USER_AGENT, HeaderValue::from_static(GEMINI_USER_AGENT));
        Ok(headers)
    }

    fn build_url(&self, model: &str) -> String {
        let model_path = if model.starts_with("models/") {
            model.to_string()
        } else {
            format!("models/{model}")
        };
        format!(
            "{}/{}:streamGenerateContent?alt=sse",
            self.base_url, model_path
        )
    }

    pub(crate) fn build_request_body(&self, request: &LlmRequest) -> Value {
        let mut body = json!({
            "contents": Self::build_contents(&request.messages),
            "generationConfig": {
                "maxOutputTokens": request.max_tokens
            }
        });

        if !request.system.is_empty() {
            body["systemInstruction"] = json!({
                "parts": [{ "text": request.system }]
            });
        }

        if !request.tools.is_empty() {
            body["tools"] = json!(Self::build_tools(&request.tools, &self.compat));
        }

        if let Some(thinking_config) = &request.thinking {
            match thinking_config {
                ThinkingConfig::Enabled { budget_tokens } => {
                    body["generationConfig"]["thinkingConfig"] =
                        Self::build_thinking_config(&request.model, *budget_tokens);
                }
                ThinkingConfig::Disabled => {
                    body["generationConfig"]["thinkingConfig"] = json!({
                        "thinkingBudget": 0
                    });
                }
            }
        }

        body
    }

    fn build_thinking_config(model: &str, budget_tokens: u32) -> Value {
        if is_gemini_3_model(model) {
            json!({
                "includeThoughts": true,
                "thinkingLevel": "HIGH"
            })
        } else {
            json!({
                "includeThoughts": true,
                "thinkingBudget": budget_tokens
            })
        }
    }

    pub(crate) fn build_contents(messages: &[Message]) -> Vec<Value> {
        let mut contents = Vec::new();
        let mut tool_names_by_id: HashMap<String, String> = HashMap::new();

        for message in messages {
            match message.role {
                Role::System => {
                    let text = text_from_blocks(&message.content);
                    if !text.is_empty() {
                        contents.push(json!({
                            "role": "user",
                            "parts": [{ "text": text }]
                        }));
                    }
                }
                Role::User | Role::Tool => {
                    let mut parts = Vec::new();
                    for block in &message.content {
                        match block {
                            ContentBlock::Text { text } if !text.is_empty() => {
                                parts.push(json!({ "text": text }));
                            }
                            ContentBlock::ToolResult {
                                tool_use_id,
                                content,
                                is_error,
                            } => {
                                let name = tool_names_by_id
                                    .get(tool_use_id)
                                    .cloned()
                                    .unwrap_or_else(|| tool_use_id.clone());
                                parts.push(json!({
                                    "functionResponse": {
                                        "id": tool_use_id,
                                        "name": name,
                                        "response": Self::build_function_response(content, *is_error)
                                    }
                                }));
                            }
                            _ => {}
                        }
                    }
                    if !parts.is_empty() {
                        contents.push(json!({
                            "role": "user",
                            "parts": parts
                        }));
                    }
                }
                Role::Assistant => {
                    let mut parts = Vec::new();
                    for block in &message.content {
                        match block {
                            ContentBlock::Text { text } if !text.is_empty() => {
                                parts.push(json!({ "text": text }));
                            }
                            ContentBlock::Thinking { .. } => {}
                            ContentBlock::ToolUse {
                                id,
                                name,
                                input,
                                extra,
                            } => {
                                if !id.is_empty() {
                                    tool_names_by_id.insert(id.clone(), name.clone());
                                }
                                let mut function_call = json!({
                                    "name": name,
                                    "args": input
                                });
                                if !id.is_empty() {
                                    function_call["id"] = json!(id);
                                }
                                let mut part = json!({ "functionCall": function_call });
                                if let Some(thought_signature) =
                                    extra.as_ref().and_then(extract_thought_signature)
                                {
                                    part["thoughtSignature"] = json!(thought_signature);
                                } else {
                                    part["thoughtSignature"] = json!(SYNTHETIC_THOUGHT_SIGNATURE);
                                }
                                parts.push(part);
                            }
                            _ => {}
                        }
                    }
                    if !parts.is_empty() {
                        contents.push(json!({
                            "role": "model",
                            "parts": parts
                        }));
                    }
                }
            }
        }

        contents
    }

    fn build_function_response(content: &str, is_error: bool) -> Value {
        if is_error {
            json!({ "error": content })
        } else {
            json!({ "output": content })
        }
    }

    pub(crate) fn build_tools(tools: &[ToolDef], compat: &ProviderCompat) -> Vec<Value> {
        let declarations: Vec<Value> = tools
            .iter()
            .map(|tool| {
                let (description, schema) = if tool.deferred {
                    (
                        format!(
                            "(Deferred) {} - Use ToolSearch to load full schema before calling.",
                            truncate_deferred_description(&tool.description)
                        ),
                        json!({
                            "type": "object",
                            "properties": {}
                        }),
                    )
                } else {
                    (tool.description.clone(), tool.input_schema.clone())
                };
                let parameters = if compat.sanitize_schema() {
                    compat::sanitize_json_schema(&schema)
                } else {
                    schema
                };
                json!({
                    "name": tool.name,
                    "description": description,
                    "parameters": parameters
                })
            })
            .collect();

        if declarations.is_empty() {
            Vec::new()
        } else {
            vec![json!({ "functionDeclarations": declarations })]
        }
    }
}

fn is_gemini_3_model(model: &str) -> bool {
    model.starts_with("gemini-3")
}

#[async_trait]
impl LlmProvider for GeminiProvider {
    async fn stream(
        &self,
        request: &LlmRequest,
    ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        self.stream_inner(request, None).await
    }

    async fn stream_with_retry_observer(
        &self,
        request: &LlmRequest,
        retry_observer: Option<RetryObserver>,
    ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        self.stream_inner(request, retry_observer).await
    }
}

impl GeminiProvider {
    async fn stream_inner(
        &self,
        request: &LlmRequest,
        retry_observer: Option<RetryObserver>,
    ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        let url = self.build_url(&request.model);
        let body = self.build_request_body(request);
        let headers = self.build_headers()?;

        tracing::debug!(target: "aion_providers", url = %url, body = %serde_json::to_string_pretty(&body).unwrap_or_default(), "outgoing Gemini request");

        let response = with_retry_if_notify_budget(
            STREAM_REQUEST_RATE_LIMIT_RETRIES,
            |error| error.is_retryable(),
            |error| match error {
                ProviderError::RateLimited { .. } => STREAM_REQUEST_RATE_LIMIT_RETRIES,
                _ => STREAM_REQUEST_CONNECTION_RETRIES,
            },
            |retry| {
                if let Some(observer) = retry_observer.as_ref() {
                    observer(retry);
                }
            },
            || post_gemini_stream_request(&self.client, &url, &headers, &body),
        )
        .await?;

        let (tx, rx) = mpsc::channel(64);
        let client = self.client.clone();
        let url_clone = url.clone();
        let headers_clone = headers.clone();
        tokio::spawn(async move {
            match process_sse_stream(response, &tx).await {
                StreamOutcome::Ok => {}
                StreamOutcome::FailedPartial(error) => {
                    let _ = tx.send(LlmEvent::Error(error.to_string())).await;
                }
                StreamOutcome::FailedEmpty(error) => {
                    if error.is_retryable() {
                        let mut backoff = Duration::from_secs(1);
                        let mut final_error = Some(error);
                        for attempt in 1..=crate::retry::MAX_STREAM_RETRIES {
                            backoff = crate::retry::backoff_sleep(attempt, backoff).await;
                            match post_gemini_stream_request(
                                &client,
                                &url_clone,
                                &headers_clone,
                                &body,
                            )
                            .await
                            {
                                Ok(response) => {
                                    let outcome = process_sse_stream(response, &tx).await;
                                    match crate::retry::evaluate_outcome(outcome, attempt) {
                                        Ok(None) => {
                                            final_error = None;
                                            break;
                                        }
                                        Ok(Some(error)) => {
                                            final_error = Some(error);
                                            break;
                                        }
                                        Err(_) => continue,
                                    }
                                }
                                Err(error) if attempt == crate::retry::MAX_STREAM_RETRIES => {
                                    final_error = Some(error);
                                    break;
                                }
                                Err(_) => continue,
                            }
                        }
                        if let Some(error) = final_error {
                            let _ = tx.send(LlmEvent::Error(error.to_string())).await;
                        }
                    } else {
                        let _ = tx.send(LlmEvent::Error(error.to_string())).await;
                    }
                }
            }
        });

        Ok(rx)
    }
}

async fn post_gemini_stream_request(
    client: &reqwest::Client,
    url: &str,
    headers: &HeaderMap,
    body: &Value,
) -> Result<reqwest::Response, ProviderError> {
    let response = client
        .post(url)
        .headers(headers.clone())
        .json(body)
        .send()
        .await
        .map_err(request_send_error)?;

    let status = response.status();
    if status.is_success() {
        if response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|content_type| content_type.to_ascii_lowercase().starts_with("text/html"))
        {
            let url = response.url().to_string();
            let body_preview = response
                .text()
                .await
                .map(|body| sanitized_preview(&body, 512))
                .unwrap_or_else(|_| "<failed to read response body>".to_string());
            return Err(ProviderError::Connection(format!(
                "expected Gemini SSE response from {url}, got text/html: {body_preview}"
            )));
        }
        return Ok(response);
    }

    let retry_after_ms = response
        .headers()
        .get(RETRY_AFTER)
        .and_then(parse_retry_after_ms)
        .unwrap_or(5000);
    let body_text = response.text().await.unwrap_or_default();
    if status.as_u16() == 429 {
        return Err(ProviderError::RateLimited { retry_after_ms });
    }

    Err(ProviderError::Api {
        status: status.as_u16(),
        message: format!("{url}: {body_text}"),
    })
}

fn parse_retry_after_ms(value: &HeaderValue) -> Option<u64> {
    value.to_str().ok()?.parse::<u64>().ok().map(|s| s * 1000)
}

fn normalize_base_url(base_url: &str) -> String {
    let trimmed = base_url.trim().trim_end_matches('/');
    if trimmed == GOOGLE_GEMINI_ROOT_URL {
        GOOGLE_GEMINI_DEFAULT_BASE_URL.to_string()
    } else if has_gemini_version_path(trimmed) {
        trimmed.to_string()
    } else {
        format!("{trimmed}/v1beta")
    }
}

fn has_gemini_version_path(base_url: &str) -> bool {
    let path = reqwest::Url::parse(base_url)
        .ok()
        .map(|url| url.path().trim_end_matches('/').to_string())
        .unwrap_or_else(|| base_url.trim_end_matches('/').to_string());
    matches!(path.rsplit('/').next(), Some("v1" | "v1alpha" | "v1beta"))
}

fn request_send_error(error: reqwest::Error) -> ProviderError {
    ProviderError::Connection(format_error_chain(&error))
}

fn format_error_chain(error: &dyn std::error::Error) -> String {
    let mut message = error.to_string();
    let mut current = error.source();
    while let Some(source) = current {
        message.push_str(": ");
        message.push_str(&source.to_string());
        current = source.source();
    }
    message
}

async fn process_sse_stream(
    response: reqwest::Response,
    tx: &mpsc::Sender<LlmEvent>,
) -> StreamOutcome {
    use futures::StreamExt;

    let response_context = response_context(&response);
    let mut buffer = String::new();
    let mut stream = response.bytes_stream();
    let mut usage = TokenUsage::default();
    let mut pending_stop = StopReason::EndTurn;
    let mut emitted_content = false;
    let mut saw_sse_frame = false;

    loop {
        let chunk = match tokio::time::timeout(STREAM_IDLE_TIMEOUT, stream.next()).await {
            Ok(Some(Ok(chunk))) => chunk,
            Ok(Some(Err(error))) => {
                return stream_failure(
                    emitted_content,
                    ProviderError::Connection(error.to_string()),
                );
            }
            Ok(None) => {
                if !buffer.trim().is_empty() {
                    let event = std::mem::take(&mut buffer);
                    saw_sse_frame = true;
                    match handle_sse_event(
                        &event,
                        tx,
                        &mut usage,
                        &mut pending_stop,
                        &mut emitted_content,
                    )
                    .await
                    {
                        SseEventOutcome::Done | SseEventOutcome::ChannelClosed => {
                            return StreamOutcome::Ok;
                        }
                        SseEventOutcome::Processed => {}
                        SseEventOutcome::Ignored => {
                            return stream_failure(
                                emitted_content,
                                ProviderError::Connection(format!(
                                    "incomplete json segment ({response_context}); preview={}",
                                    sanitized_preview(&event, 512)
                                )),
                            );
                        }
                    }
                }
                if !saw_sse_frame {
                    return StreamOutcome::FailedEmpty(ProviderError::Connection(
                        "SSE stream closed before any Gemini event".to_string(),
                    ));
                }
                let _ = tx
                    .send(LlmEvent::Done {
                        stop_reason: pending_stop,
                        usage,
                    })
                    .await;
                return StreamOutcome::Ok;
            }
            Err(_) => {
                return stream_failure(
                    emitted_content,
                    ProviderError::Connection("idle timeout waiting for Gemini SSE".to_string()),
                );
            }
        };

        buffer.push_str(&String::from_utf8_lossy(&chunk));

        while let Some(event) = pop_next_sse_event(&mut buffer) {
            saw_sse_frame = true;
            match handle_sse_event(
                &event,
                tx,
                &mut usage,
                &mut pending_stop,
                &mut emitted_content,
            )
            .await
            {
                SseEventOutcome::Done | SseEventOutcome::ChannelClosed => return StreamOutcome::Ok,
                SseEventOutcome::Processed | SseEventOutcome::Ignored => {}
            }
        }
    }
}

fn response_context(response: &reqwest::Response) -> String {
    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("<missing>");
    format!(
        "url={}, status={}, content-type={}",
        response.url(),
        response.status(),
        content_type
    )
}

fn sanitized_preview(input: &str, max_chars: usize) -> String {
    let preview: String = input
        .chars()
        .take(max_chars)
        .map(|ch| {
            if ch.is_control() && ch != '\n' && ch != '\r' && ch != '\t' {
                ' '
            } else {
                ch
            }
        })
        .collect();
    let normalized = preview
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace("Bearer ", "Bearer <redacted> ");
    let normalized = redact_prefixed_token(&normalized, "sk-");
    redact_prefixed_token(&normalized, "AIza")
}

fn redact_prefixed_token(input: &str, prefix: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(index) = rest.find(prefix) {
        output.push_str(&rest[..index]);
        output.push_str(prefix);
        output.push_str("<redacted>");
        let token_start = index + prefix.len();
        let token_len = rest[token_start..]
            .chars()
            .take_while(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
            .map(char::len_utf8)
            .sum::<usize>();
        rest = &rest[token_start + token_len..];
    }
    output.push_str(rest);
    output
}

enum SseEventOutcome {
    Processed,
    Ignored,
    Done,
    ChannelClosed,
}

async fn handle_sse_event(
    event: &str,
    tx: &mpsc::Sender<LlmEvent>,
    usage: &mut TokenUsage,
    pending_stop: &mut StopReason,
    emitted_content: &mut bool,
) -> SseEventOutcome {
    let data = sse_data_payload(event);
    if data.trim().is_empty() {
        return SseEventOutcome::Ignored;
    }
    if data.trim() == "[DONE]" {
        let _ = tx
            .send(LlmEvent::Done {
                stop_reason: *pending_stop,
                usage: usage.clone(),
            })
            .await;
        return SseEventOutcome::Done;
    }

    let Ok((events, chunk_usage, stop_reason)) = parse_sse_data_checked(&data) else {
        return SseEventOutcome::Ignored;
    };
    if let Some(chunk_usage) = chunk_usage {
        *usage = chunk_usage;
    }
    if let Some(stop_reason) = stop_reason {
        *pending_stop = stop_reason;
    }
    for event in events {
        if tx.send(event).await.is_err() {
            return SseEventOutcome::ChannelClosed;
        }
        *emitted_content = true;
    }

    SseEventOutcome::Processed
}

fn pop_next_sse_event(buffer: &mut String) -> Option<String> {
    let lf = buffer.find("\n\n").map(|idx| (idx, 2));
    let crlf = buffer.find("\r\n\r\n").map(|idx| (idx, 4));
    let (event_end, separator_len) = match (lf, crlf) {
        (Some(lf), Some(crlf)) => {
            if lf.0 <= crlf.0 {
                lf
            } else {
                crlf
            }
        }
        (Some(found), None) | (None, Some(found)) => found,
        (None, None) => return None,
    };

    let event = buffer[..event_end].to_string();
    buffer.drain(..event_end + separator_len);
    Some(event)
}

fn sse_data_payload(event: &str) -> String {
    event
        .lines()
        .filter_map(|line| {
            let line = line.trim_end_matches('\r');
            line.strip_prefix("data:")
                .map(|data| data.strip_prefix(' ').unwrap_or(data))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn stream_failure(emitted_content: bool, error: ProviderError) -> StreamOutcome {
    if emitted_content {
        StreamOutcome::FailedPartial(error)
    } else {
        StreamOutcome::FailedEmpty(error)
    }
}

#[cfg(test)]
fn parse_sse_data(data: &str) -> (Vec<LlmEvent>, Option<TokenUsage>, Option<StopReason>) {
    parse_sse_data_checked(data).unwrap_or_else(|_| (Vec::new(), None, None))
}

fn parse_sse_data_checked(
    data: &str,
) -> Result<(Vec<LlmEvent>, Option<TokenUsage>, Option<StopReason>), serde_json::Error> {
    let json: Value = serde_json::from_str(data)?;

    let usage = parse_usage(&json);
    let mut events = Vec::new();
    let mut stop_reason = None;

    if let Some(candidate) = json
        .get("candidates")
        .and_then(Value::as_array)
        .and_then(|candidates| candidates.first())
    {
        if let Some(parts) = candidate
            .get("content")
            .and_then(|content| content.get("parts"))
            .and_then(Value::as_array)
        {
            for part in parts {
                if let Some(text) = part.get("text").and_then(Value::as_str)
                    && !text.is_empty()
                {
                    if part
                        .get("thought")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                    {
                        events.push(LlmEvent::ThinkingDelta(text.to_string()));
                    } else {
                        events.push(LlmEvent::TextDelta(text.to_string()));
                    }
                }
                if let Some(function_call) = part.get("functionCall") {
                    let name = function_call
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    if name.is_empty() {
                        continue;
                    }
                    let input = function_call
                        .get("args")
                        .cloned()
                        .unwrap_or_else(|| json!({}));
                    let id = function_call
                        .get("id")
                        .and_then(Value::as_str)
                        .filter(|id| !id.is_empty())
                        .map(ToString::to_string)
                        .unwrap_or_else(next_tool_call_id);
                    let extra = part.get("thoughtSignature").cloned().map(|signature| {
                        json!({
                            "thoughtSignature": signature
                        })
                    });
                    events.push(LlmEvent::ToolUse {
                        id,
                        name,
                        input,
                        extra,
                    });
                    stop_reason = Some(StopReason::ToolUse);
                }
            }
        }

        if let Some(reason) = candidate.get("finishReason").and_then(Value::as_str) {
            stop_reason = Some(match reason {
                "MAX_TOKENS" => StopReason::MaxTokens,
                "STOP" if matches!(stop_reason, Some(StopReason::ToolUse)) => StopReason::ToolUse,
                "STOP" => StopReason::EndTurn,
                _ => StopReason::EndTurn,
            });
        }
    }

    Ok((events, usage, stop_reason))
}

fn parse_usage(json: &Value) -> Option<TokenUsage> {
    let usage = json.get("usageMetadata")?;
    Some(TokenUsage {
        input_tokens: usage
            .get("promptTokenCount")
            .and_then(Value::as_u64)
            .unwrap_or_default(),
        output_tokens: usage
            .get("candidatesTokenCount")
            .and_then(Value::as_u64)
            .unwrap_or_default(),
        cache_creation_tokens: 0,
        cache_read_tokens: usage
            .get("cachedContentTokenCount")
            .and_then(Value::as_u64)
            .unwrap_or_default(),
    })
}

fn next_tool_call_id() -> String {
    let id = TOOL_CALL_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("gemini_call_{id}")
}

fn text_from_blocks(blocks: &[ContentBlock]) -> String {
    blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn extract_thought_signature(extra: &Value) -> Option<&str> {
    extra.get("thoughtSignature").and_then(Value::as_str)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::LlmProvider;
    use serde_json::json;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn make_request(model: &str) -> LlmRequest {
        LlmRequest {
            session_id: None,
            model: model.to_string(),
            system: "You are concise.".to_string(),
            messages: vec![Message::new(
                Role::User,
                vec![ContentBlock::Text {
                    text: "Hello".to_string(),
                }],
            )],
            tools: vec![],
            max_tokens: 128,
            thinking: None,
            reasoning_effort: None,
        }
    }

    async fn collect_events(mut rx: mpsc::Receiver<LlmEvent>) -> Vec<LlmEvent> {
        let mut events = Vec::new();
        while let Some(event) = rx.recv().await {
            events.push(event);
        }
        events
    }

    #[test]
    fn build_url_uses_native_stream_endpoint() {
        let provider = GeminiProvider::new(
            "key",
            "https://generativelanguage.googleapis.com/v1beta",
            ProviderCompat::gemini_defaults(),
        );

        assert_eq!(
            provider.build_url("gemini-3.1-pro-preview"),
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-3.1-pro-preview:streamGenerateContent?alt=sse"
        );
    }

    #[test]
    fn build_url_normalizes_google_root_base_url() {
        let provider = GeminiProvider::new(
            "key",
            "https://generativelanguage.googleapis.com",
            ProviderCompat::gemini_defaults(),
        );

        assert_eq!(
            provider.build_url("gemini-3.1-pro-preview"),
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-3.1-pro-preview:streamGenerateContent?alt=sse"
        );
    }

    #[test]
    fn build_url_adds_native_version_for_custom_gateway_root() {
        let provider = GeminiProvider::new(
            "key",
            "https://sub2api.taichuy.com",
            ProviderCompat::gemini_defaults(),
        );

        assert_eq!(
            provider.build_url("gemini-3.1-pro-preview"),
            "https://sub2api.taichuy.com/v1beta/models/gemini-3.1-pro-preview:streamGenerateContent?alt=sse"
        );
    }

    #[test]
    fn build_url_preserves_custom_gateway_version_path() {
        let provider = GeminiProvider::new(
            "key",
            "https://sub2api.taichuy.com/v1beta",
            ProviderCompat::gemini_defaults(),
        );

        assert_eq!(
            provider.build_url("gemini-3.1-pro-preview"),
            "https://sub2api.taichuy.com/v1beta/models/gemini-3.1-pro-preview:streamGenerateContent?alt=sse"
        );
    }

    #[test]
    fn parse_text_chunk() {
        let data = r#"{"candidates":[{"content":{"parts":[{"text":"Hello"}],"role":"model"},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":3,"candidatesTokenCount":2}}"#;
        let (events, usage, stop_reason) = parse_sse_data(data);

        assert!(matches!(&events[0], LlmEvent::TextDelta(text) if text == "Hello"));
        assert_eq!(usage.unwrap().input_tokens, 3);
        assert_eq!(stop_reason, Some(StopReason::EndTurn));
    }

    #[test]
    fn parse_function_call_chunk() {
        let data = r#"{"candidates":[{"content":{"parts":[{"thoughtSignature":"sig-1","functionCall":{"id":"call-1","name":"Read","args":{"path":"README.md"}}}],"role":"model"},"finishReason":"STOP"}]}"#;
        let (events, _, stop_reason) = parse_sse_data(data);

        match &events[0] {
            LlmEvent::ToolUse {
                id,
                name,
                input,
                extra,
            } => {
                assert_eq!(id, "call-1");
                assert_eq!(name, "Read");
                assert_eq!(input["path"], "README.md");
                assert_eq!(extra.as_ref().unwrap()["thoughtSignature"], "sig-1");
            }
            other => panic!("expected tool use, got {other:?}"),
        }
        assert_eq!(stop_reason, Some(StopReason::ToolUse));
    }

    #[test]
    fn parse_thought_chunk() {
        let data = r#"{"candidates":[{"content":{"parts":[{"text":"thinking","thought":true}],"role":"model"}}]}"#;
        let (events, _, _) = parse_sse_data(data);

        assert!(matches!(&events[0], LlmEvent::ThinkingDelta(text) if text == "thinking"));
    }

    #[test]
    fn pop_sse_event_accepts_crlf_separator() {
        let chunk = json!({
            "candidates": [{
                "content": { "parts": [{ "text": "Hello" }], "role": "model" }
            }]
        })
        .to_string();
        let mut buffer = format!("data: {chunk}\r\n\r\n");

        let event = pop_next_sse_event(&mut buffer).unwrap();

        assert!(buffer.is_empty());
        assert_eq!(sse_data_payload(&event), chunk);
    }

    #[tokio::test]
    async fn handle_sse_event_processes_final_event_without_blank_line() {
        let chunk = json!({
            "candidates": [{
                "content": { "parts": [{ "text": "tail" }], "role": "model" },
                "finishReason": "STOP"
            }]
        })
        .to_string();
        let (tx, mut rx) = mpsc::channel(4);
        let mut usage = TokenUsage::default();
        let mut pending_stop = StopReason::EndTurn;
        let mut emitted_content = false;

        let outcome = handle_sse_event(
            &format!("data: {chunk}"),
            &tx,
            &mut usage,
            &mut pending_stop,
            &mut emitted_content,
        )
        .await;

        assert!(matches!(outcome, SseEventOutcome::Processed));
        assert!(emitted_content);
        assert!(matches!(rx.recv().await, Some(LlmEvent::TextDelta(text)) if text == "tail"));
    }

    #[test]
    fn build_contents_uses_native_function_response_shape() {
        let messages = vec![
            Message::new(
                Role::Assistant,
                vec![ContentBlock::ToolUse {
                    id: "call-1".into(),
                    name: "Read".into(),
                    input: json!({ "path": "README.md" }),
                    extra: None,
                }],
            ),
            Message::new(
                Role::User,
                vec![ContentBlock::ToolResult {
                    tool_use_id: "call-1".into(),
                    content: "file contents".into(),
                    is_error: false,
                }],
            ),
        ];

        let contents = GeminiProvider::build_contents(&messages);

        assert_eq!(contents[0]["parts"][0]["functionCall"]["id"], "call-1");
        assert_eq!(
            contents[0]["parts"][0]["thoughtSignature"],
            SYNTHETIC_THOUGHT_SIGNATURE
        );
        assert_eq!(contents[1]["parts"][0]["functionResponse"]["id"], "call-1");
        assert_eq!(
            contents[1]["parts"][0]["functionResponse"]["response"],
            json!({ "output": "file contents" })
        );
    }

    #[test]
    fn build_request_body_maps_thinking_config() {
        let mut request = make_request("gemini-test");
        request.thinking = Some(ThinkingConfig::Enabled {
            budget_tokens: 1024,
        });
        let provider = GeminiProvider::new(
            "key",
            "https://generativelanguage.googleapis.com/v1beta",
            ProviderCompat::gemini_defaults(),
        );

        let body = provider.build_request_body(&request);

        assert_eq!(
            body["generationConfig"]["thinkingConfig"],
            json!({
                "includeThoughts": true,
                "thinkingBudget": 1024
            })
        );
    }

    #[test]
    fn build_request_body_uses_thinking_level_for_gemini_3() {
        let mut request = make_request("gemini-3.1-pro-preview");
        request.thinking = Some(ThinkingConfig::Enabled {
            budget_tokens: 8192,
        });
        let provider = GeminiProvider::new(
            "key",
            "https://generativelanguage.googleapis.com/v1beta",
            ProviderCompat::gemini_defaults(),
        );

        let body = provider.build_request_body(&request);

        assert_eq!(
            body["generationConfig"]["thinkingConfig"],
            json!({
                "includeThoughts": true,
                "thinkingLevel": "HIGH"
            })
        );
    }

    #[test]
    fn build_tools_uses_function_declarations() {
        let tools = vec![ToolDef {
            name: "Read".into(),
            description: "Read a file".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" }
                }
            }),
            deferred: false,
        }];

        let result = GeminiProvider::build_tools(&tools, &ProviderCompat::gemini_defaults());
        assert_eq!(result[0]["functionDeclarations"][0]["name"], "Read");
        assert_eq!(
            result[0]["functionDeclarations"][0]["parameters"]["properties"]["path"]["type"],
            "string"
        );
    }

    #[tokio::test]
    async fn stream_retries_transient_503_before_response() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/v1beta/models/gemini-test:streamGenerateContent"))
            .respond_with(ResponseTemplate::new(503).set_body_string("service unavailable"))
            .up_to_n_times(1)
            .with_priority(1)
            .mount(&server)
            .await;

        let chunk = json!({
            "candidates": [{
                "content": { "parts": [{ "text": "Recovered" }], "role": "model" },
                "finishReason": "STOP"
            }],
            "usageMetadata": {
                "promptTokenCount": 3,
                "candidatesTokenCount": 1
            }
        })
        .to_string();
        let sse_body = format!("data: {chunk}\n\n");

        Mock::given(method("POST"))
            .and(path("/v1beta/models/gemini-test:streamGenerateContent"))
            .and(header("x-goog-api-key", "test-key"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(sse_body, "text/event-stream"))
            .with_priority(2)
            .mount(&server)
            .await;

        let provider = GeminiProvider::new(
            "test-key",
            &format!("{}/v1beta", server.uri()),
            ProviderCompat::gemini_defaults(),
        );
        let rx = provider.stream(&make_request("gemini-test")).await.unwrap();
        let events = collect_events(rx).await;

        assert_eq!(events.len(), 2, "expected retry success, got: {events:?}");
        assert!(matches!(&events[0], LlmEvent::TextDelta(text) if text == "Recovered"));
        assert!(matches!(
            &events[1],
            LlmEvent::Done {
                stop_reason: StopReason::EndTurn,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn stream_retries_empty_stream_before_content() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/v1beta/models/gemini-test:streamGenerateContent"))
            .respond_with(ResponseTemplate::new(200).set_body_raw("", "text/event-stream"))
            .up_to_n_times(1)
            .with_priority(1)
            .mount(&server)
            .await;

        let chunk = json!({
            "candidates": [{
                "content": { "parts": [{ "text": "After empty stream" }], "role": "model" },
                "finishReason": "STOP"
            }]
        })
        .to_string();
        let sse_body = format!("data: {chunk}\n\n");

        Mock::given(method("POST"))
            .and(path("/v1beta/models/gemini-test:streamGenerateContent"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(sse_body, "text/event-stream"))
            .with_priority(2)
            .mount(&server)
            .await;

        let provider = GeminiProvider::new(
            "test-key",
            &format!("{}/v1beta", server.uri()),
            ProviderCompat::gemini_defaults(),
        );
        let rx = provider.stream(&make_request("gemini-test")).await.unwrap();
        let events = collect_events(rx).await;

        assert_eq!(events.len(), 2, "expected retry success, got: {events:?}");
        assert!(matches!(&events[0], LlmEvent::TextDelta(text) if text == "After empty stream"));
    }
}
