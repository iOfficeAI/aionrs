// AWS Bedrock provider for Claude models.
// Uses AWS SigV4 authentication and AWS event stream binary framing.

use async_trait::async_trait;
use aws_credential_types::Credentials;
use aws_sigv4::http_request::{
    self as sigv4_http, PayloadChecksumKind, SignableBody, SignableRequest, SignatureLocation,
    SigningSettings,
};
use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderValue};
use serde_json::{Value, json};
use std::time::SystemTime;
use tokio::sync::mpsc;

use base64::Engine as _;

use aion_types::llm::{LlmEvent, LlmRequest, ThinkingConfig};
use aion_types::message::{StopReason, TokenUsage};

use super::anthropic_shared;
use crate::{
    LlmProvider, ProviderError, ProviderFailure, ProviderFailurePhase, ProviderStreamItem,
    ProviderStreamReceiver, provider_api_error, provider_failure_from_error,
    retry_after_ms_from_headers,
};
use aion_config::compat::{self, ProviderCompat};

pub struct BedrockProvider {
    client: reqwest::Client,
    region: String,
    credentials: AwsCredentials,
    cache_enabled: bool,
    compat: ProviderCompat,
}

#[derive(Debug, Clone)]
pub enum AwsCredentials {
    Explicit {
        access_key_id: String,
        secret_access_key: String,
        session_token: Option<String>,
    },
    Profile(String),
    Environment,
}

impl BedrockProvider {
    pub fn new(
        region: &str,
        credentials: AwsCredentials,
        cache_enabled: bool,
        compat: ProviderCompat,
    ) -> Self {
        Self {
            client: reqwest::Client::new(),
            region: region.to_string(),
            credentials,
            cache_enabled,
            compat,
        }
    }

    fn build_request_body(&self, request: &LlmRequest) -> Value {
        let system = if self.cache_enabled {
            json!([{
                "type": "text",
                "text": &request.system,
                "cache_control": { "type": "ephemeral" }
            }])
        } else {
            json!(&request.system)
        };

        let mut body = json!({
            "anthropic_version": "bedrock-2023-05-31",
            "max_tokens": request.max_tokens,
            "system": system,
            "messages": anthropic_shared::build_messages(&request.messages, &self.compat)
        });

        if !request.tools.is_empty() {
            let mut tools = anthropic_shared::build_tools(&request.tools);
            if self.compat.sanitize_schema() {
                for tool in &mut tools {
                    if let Some(schema) = tool.get("input_schema").cloned() {
                        tool["input_schema"] = compat::sanitize_json_schema(&schema);
                    }
                }
            }
            if self.cache_enabled
                && let Some(last) = tools.last_mut()
            {
                last["cache_control"] = json!({ "type": "ephemeral" });
            }
            body["tools"] = json!(tools);
        }

        if let Some(ThinkingConfig::Enabled { budget_tokens }) = &request.thinking {
            body["thinking"] = json!({
                "type": "enabled",
                "budget_tokens": budget_tokens
            });
        }

        body
    }

    fn build_url(&self, model: &str) -> String {
        format!(
            "https://bedrock-runtime.{}.amazonaws.com/model/{}/invoke-with-response-stream",
            self.region, model
        )
    }

    fn resolve_credentials(&self) -> Result<Credentials, ProviderError> {
        match &self.credentials {
            AwsCredentials::Explicit {
                access_key_id,
                secret_access_key,
                session_token,
            } => Ok(Credentials::new(
                access_key_id,
                secret_access_key,
                session_token.clone(),
                None,
                "aionrs",
            )),
            AwsCredentials::Profile(profile) => Self::credentials_from_sdk(Some(profile.clone())),
            AwsCredentials::Environment => Self::credentials_from_sdk(None),
        }
    }

    fn credentials_from_sdk(profile: Option<String>) -> Result<Credentials, ProviderError> {
        // Use a short-lived tokio runtime to resolve credentials synchronously.
        // This is called once per LLM request so the overhead is acceptable.
        let rt = tokio::runtime::Handle::try_current();

        let resolve = async move {
            let mut loader = aws_config::defaults(aws_config::BehaviorVersion::latest());
            if let Some(p) = profile {
                loader = loader.profile_name(p);
            }
            let config = loader.load().await;
            let provider = config.credentials_provider().ok_or_else(|| {
                ProviderError::Connection(
                    "No AWS credentials found. Set AWS_ACCESS_KEY_ID/AWS_SECRET_ACCESS_KEY, \
                     AWS_PROFILE, or configure credentials in ~/.aws/credentials"
                        .into(),
                )
            })?;

            use aws_credential_types::provider::ProvideCredentials;
            let creds = provider
                .provide_credentials()
                .await
                .map_err(|e| ProviderError::Connection(format!("AWS credential error: {}", e)))?;

            Ok(Credentials::new(
                creds.access_key_id(),
                creds.secret_access_key(),
                creds.session_token().map(|s| s.to_string()),
                creds.expiry(),
                "aionrs-sdk",
            ))
        };

        match rt {
            Ok(_handle) => {
                // Already inside a tokio runtime — use spawn_blocking to avoid nested block_on
                std::thread::scope(|s| {
                    s.spawn(|| {
                        tokio::runtime::Runtime::new()
                            .map_err(|e| {
                                ProviderError::Connection(format!("Runtime error: {}", e))
                            })?
                            .block_on(resolve)
                    })
                    .join()
                    .unwrap()
                })
            }
            Err(_) => {
                // No runtime — safe to create one
                tokio::runtime::Runtime::new()
                    .map_err(|e| ProviderError::Connection(format!("Runtime error: {}", e)))?
                    .block_on(resolve)
            }
        }
    }

    fn sign_request(
        &self,
        method: &str,
        url: &str,
        headers: &HeaderMap,
        body: &[u8],
        credentials: &Credentials,
    ) -> Result<HeaderMap, ProviderError> {
        let mut signing_settings = SigningSettings::default();
        signing_settings.payload_checksum_kind = PayloadChecksumKind::XAmzSha256;
        signing_settings.signature_location = SignatureLocation::Headers;

        let identity = credentials.clone().into();
        let signing_params = aws_sigv4::sign::v4::SigningParams::builder()
            .identity(&identity)
            .region(&self.region)
            .name("bedrock")
            .time(SystemTime::now())
            .settings(signing_settings)
            .build()
            .map_err(|e| ProviderError::Connection(format!("SigV4 params error: {}", e)))?;

        // Build header pairs for signing
        let header_pairs: Vec<(&str, &str)> = headers
            .iter()
            .filter_map(|(name, value)| value.to_str().ok().map(|v| (name.as_str(), v)))
            .collect();

        let signable_request = SignableRequest::new(
            method,
            url,
            header_pairs.into_iter(),
            SignableBody::Bytes(body),
        )
        .map_err(|e| ProviderError::Connection(format!("Signable request error: {}", e)))?;

        let (signing_instructions, _signature) =
            sigv4_http::sign(signable_request, &signing_params.into())
                .map_err(|e| ProviderError::Connection(format!("SigV4 signing error: {}", e)))?
                .into_parts();

        let mut signed_headers = headers.clone();
        for (name, value) in signing_instructions.headers() {
            signed_headers.insert(
                reqwest::header::HeaderName::from_bytes(name.as_bytes())
                    .map_err(|e| ProviderError::Connection(format!("Header name error: {}", e)))?,
                HeaderValue::from_str(value)
                    .map_err(|e| ProviderError::Connection(format!("Header value error: {}", e)))?,
            );
        }

        Ok(signed_headers)
    }
}

#[async_trait]
impl LlmProvider for BedrockProvider {
    async fn stream(
        &self,
        request: &LlmRequest,
    ) -> Result<ProviderStreamReceiver, ProviderFailure> {
        let url = self.build_url(&request.model);
        let body = self.build_request_body(request);
        let model = Some(request.model.clone());

        tracing::debug!(target: "aion_providers", body = %serde_json::to_string_pretty(&body).unwrap_or_default(), "outgoing request");

        let body_bytes = serde_json::to_vec(&body)
            .map_err(|e| ProviderError::Connection(format!("JSON serialize error: {}", e)))
            .map_err(|error| {
                provider_failure_from_error(
                    "bedrock",
                    model.clone(),
                    error,
                    ProviderFailurePhase::BeforeFirstDelta,
                )
            })?;

        let credentials = self.resolve_credentials().map_err(|error| {
            provider_failure_from_error(
                "bedrock",
                model.clone(),
                error,
                ProviderFailurePhase::BeforeFirstDelta,
            )
        })?;

        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        let signed_headers = self
            .sign_request("POST", &url, &headers, &body_bytes, &credentials)
            .map_err(|error| {
                provider_failure_from_error(
                    "bedrock",
                    model.clone(),
                    error,
                    ProviderFailurePhase::BeforeFirstDelta,
                )
            })?;

        let response = self
            .client
            .post(&url)
            .headers(signed_headers)
            .body(body_bytes)
            .send()
            .await
            .map_err(ProviderError::from)
            .map_err(|error| {
                provider_failure_from_error(
                    "bedrock",
                    model.clone(),
                    error,
                    ProviderFailurePhase::BeforeFirstDelta,
                )
            })?;

        let status = response.status();
        if !status.is_success() {
            let retry_after_ms = retry_after_ms_from_headers(response.headers());
            let body_text = response.text().await.unwrap_or_default();
            let message = format_bedrock_error(status.as_u16(), &body_text);
            return Err(provider_failure_from_error(
                "bedrock",
                model,
                provider_api_error(status.as_u16(), message, retry_after_ms),
                ProviderFailurePhase::BeforeFirstDelta,
            ));
        }

        let (tx, rx) = mpsc::channel(64);
        let stream_model = model.clone();

        // AWS event stream uses binary framing
        tokio::spawn(async move {
            match process_aws_event_stream(response, &tx).await {
                anthropic_shared::StreamOutcome::Ok => {}
                anthropic_shared::StreamOutcome::FailedPartial(e) => {
                    // Bedrock retry requires re-signing; not implemented yet.
                    let failure = provider_failure_from_error(
                        "bedrock",
                        stream_model.clone(),
                        e,
                        ProviderFailurePhase::AfterFirstDelta,
                    );
                    let _ = tx.send(Err(failure)).await;
                }
                anthropic_shared::StreamOutcome::FailedEmpty(e) => {
                    let failure = provider_failure_from_error(
                        "bedrock",
                        stream_model.clone(),
                        e,
                        ProviderFailurePhase::BeforeFirstDelta,
                    );
                    let _ = tx.send(Err(failure)).await;
                }
            }
        });

        Ok(rx)
    }
}

/// Process the AWS event stream (binary framed) from Bedrock
async fn process_aws_event_stream(
    response: reqwest::Response,
    tx: &mpsc::Sender<ProviderStreamItem>,
) -> anthropic_shared::StreamOutcome {
    use futures::StreamExt;

    let mut state = anthropic_shared::StreamState::new();
    let mut buffer = Vec::new();
    let mut stream = response.bytes_stream();
    let mut emitted_content = false;

    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(c) => c,
            Err(e) => {
                let err = ProviderError::Connection(e.to_string());
                return if emitted_content {
                    anthropic_shared::StreamOutcome::FailedPartial(err)
                } else {
                    anthropic_shared::StreamOutcome::FailedEmpty(err)
                };
            }
        };
        buffer.extend_from_slice(&chunk);

        // Parse complete AWS event stream messages from buffer
        while let Some((event_data, consumed)) = parse_aws_event(&buffer) {
            buffer = buffer[consumed..].to_vec();

            if let Some(payload) = event_data {
                // The payload contains an SSE-like structure with "bytes" field
                let wrapper = match serde_json::from_slice::<Value>(&payload) {
                    Ok(wrapper) => wrapper,
                    Err(e) => {
                        return bedrock_parse_failure(
                            format!("Bedrock event wrapper JSON parse error: {e}"),
                            emitted_content,
                        );
                    }
                };

                // Bedrock wraps the payload in {"bytes": "base64-encoded-data"}.
                let Some(b64) = wrapper["bytes"].as_str() else {
                    continue;
                };
                let decoded = match base64::engine::general_purpose::STANDARD.decode(b64) {
                    Ok(decoded) => decoded,
                    Err(e) => {
                        return bedrock_parse_failure(
                            format!("Bedrock event payload base64 decode error: {e}"),
                            emitted_content,
                        );
                    }
                };
                let inner = match String::from_utf8(decoded) {
                    Ok(inner) => inner,
                    Err(e) => {
                        return bedrock_parse_failure(
                            format!("Bedrock event payload UTF-8 decode error: {e}"),
                            emitted_content,
                        );
                    }
                };
                tracing::debug!(target: "aion_providers", chunk = %inner, "bedrock event chunk");

                // Inner payload is JSON with event type hints.
                let json_val = match serde_json::from_str::<Value>(&inner) {
                    Ok(json_val) => json_val,
                    Err(e) => {
                        return bedrock_parse_failure(
                            format!("Bedrock inner JSON parse error: {e}"),
                            emitted_content,
                        );
                    }
                };
                let event_type = json_val["type"].as_str().unwrap_or("");
                let items = anthropic_shared::parse_sse_data(event_type, &inner, &mut state);
                for item in items {
                    let event = match item {
                        Ok(event) => event,
                        Err(error) => {
                            return if emitted_content {
                                anthropic_shared::StreamOutcome::FailedPartial(error)
                            } else {
                                anthropic_shared::StreamOutcome::FailedEmpty(error)
                            };
                        }
                    };
                    if matches!(
                        event,
                        LlmEvent::TextDelta(_)
                            | LlmEvent::ThinkingDelta(_)
                            | LlmEvent::ThinkingSignature(_)
                            | LlmEvent::ToolUse { .. }
                    ) {
                        emitted_content = true;
                    }
                    if tx.send(Ok(event)).await.is_err() {
                        return anthropic_shared::StreamOutcome::Ok;
                    }
                }
            }
        }
    }

    // If we haven't sent a Done event, send one now
    if state.input_tokens > 0 || state.output_tokens > 0 {
        let _ = tx
            .send(Ok(LlmEvent::Done {
                stop_reason: StopReason::EndTurn,
                usage: TokenUsage {
                    input_tokens: state.input_tokens,
                    output_tokens: state.output_tokens,
                    cache_creation_tokens: state.cache_creation_tokens,
                    cache_read_tokens: state.cache_read_tokens,
                },
            }))
            .await;
    }

    anthropic_shared::StreamOutcome::Ok
}

fn bedrock_parse_failure(
    message: String,
    emitted_content: bool,
) -> anthropic_shared::StreamOutcome {
    let error = ProviderError::Parse(message);
    if emitted_content {
        anthropic_shared::StreamOutcome::FailedPartial(error)
    } else {
        anthropic_shared::StreamOutcome::FailedEmpty(error)
    }
}

/// Parse one AWS event stream message from the buffer.
/// Returns (Some(payload), bytes_consumed) if a complete message is found,
/// or None if more data is needed.
///
/// AWS event stream binary format:
/// - Prelude: total_len (4 bytes, big-endian) + headers_len (4 bytes) + prelude_crc (4 bytes)
/// - Headers: variable length
/// - Payload: variable length
/// - Message CRC: 4 bytes
fn parse_aws_event(buffer: &[u8]) -> Option<(Option<Vec<u8>>, usize)> {
    if buffer.len() < 12 {
        return None; // Need at least the prelude
    }

    let total_len = u32::from_be_bytes([buffer[0], buffer[1], buffer[2], buffer[3]]) as usize;
    let headers_len = u32::from_be_bytes([buffer[4], buffer[5], buffer[6], buffer[7]]) as usize;

    if buffer.len() < total_len {
        return None; // Incomplete message
    }

    // Prelude is 12 bytes (total_len + headers_len + prelude_crc)
    // Payload starts after prelude + headers
    let payload_start = 12 + headers_len;
    // Payload ends 4 bytes before total_len (message CRC)
    let payload_end = total_len - 4;

    if payload_start <= payload_end {
        let payload = buffer[payload_start..payload_end].to_vec();
        Some((Some(payload), total_len))
    } else {
        // Empty payload (e.g., initial response event)
        Some((None, total_len))
    }
}

/// Format Bedrock error responses with actionable hints
fn format_bedrock_error(status: u16, body: &str) -> String {
    // Try to extract the AWS error type from the response
    let error_type = serde_json::from_str::<Value>(body).ok().and_then(|v| {
        v.get("__type")
            .or_else(|| v.get("type"))
            .and_then(|t| t.as_str().map(String::from))
    });

    let hint = match status {
        403 => Some(
            "Check IAM permissions: the role/user needs bedrock:InvokeModelWithResponseStream. \
             Also verify the model is enabled in the Bedrock console for your account.",
        ),
        404 => Some(
            "Model not found in this region. Verify the model ID and that it's available in \
             your configured AWS region.",
        ),
        400 => {
            if body.contains("schema") || body.contains("Schema") {
                Some(
                    "Request schema validation failed. If using tools, try enabling sanitize_schema=true in [providers.bedrock.compat].",
                )
            } else {
                Some("Bad request — check model parameters and message format.")
            }
        }
        503 | 529 => Some(
            "Service overloaded or throttled. You may have exceeded your provisioned throughput quota. \
             Retry after a moment or request a quota increase.",
        ),
        _ => None,
    };

    let type_info = error_type.map(|t| format!(" [{}]", t)).unwrap_or_default();

    match hint {
        Some(h) => format!("{}{}\nHint: {}", body, type_info, h),
        None => format!("{}{}", body, type_info),
    }
}

/// Build AwsCredentials from aion-config's BedrockConfig
pub fn credentials_from_config(bc: &aion_config::config::BedrockConfig) -> AwsCredentials {
    if let (Some(key_id), Some(secret)) = (&bc.access_key_id, &bc.secret_access_key) {
        AwsCredentials::Explicit {
            access_key_id: key_id.clone(),
            secret_access_key: secret.clone(),
            session_token: bc.session_token.clone(),
        }
    } else if let Some(profile) = &bc.profile {
        AwsCredentials::Profile(profile.clone())
    } else {
        AwsCredentials::Environment
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use base64::Engine;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    fn aws_event_frame(payload: &[u8]) -> Vec<u8> {
        let total_len = 12 + payload.len() + 4;
        let mut frame = Vec::with_capacity(total_len);
        frame.extend_from_slice(&(total_len as u32).to_be_bytes());
        frame.extend_from_slice(&0_u32.to_be_bytes());
        frame.extend_from_slice(&0_u32.to_be_bytes());
        frame.extend_from_slice(payload);
        frame.extend_from_slice(&0_u32.to_be_bytes());
        frame
    }

    async fn response_for_body(body: Vec<u8>) -> reqwest::Response {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = [0_u8; 4096];
            let _ = stream.read(&mut buf).await.unwrap();
            let headers = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/vnd.amazon.eventstream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                body.len()
            );
            stream.write_all(headers.as_bytes()).await.unwrap();
            stream.write_all(&body).await.unwrap();
        });

        reqwest::Client::new()
            .get(format!("http://{addr}"))
            .send()
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn bedrock_invalid_event_wrapper_returns_parse_failure() {
        let response = response_for_body(aws_event_frame(b"{not-json}")).await;
        let (tx, _rx) = mpsc::channel(1);

        let outcome = process_aws_event_stream(response, &tx).await;

        assert!(matches!(
            outcome,
            anthropic_shared::StreamOutcome::FailedEmpty(ProviderError::Parse(_))
        ));
    }

    #[tokio::test]
    async fn bedrock_invalid_inner_json_returns_parse_failure() {
        let inner = base64::engine::general_purpose::STANDARD.encode("{not-json}");
        let wrapper = serde_json::json!({ "bytes": inner }).to_string();
        let response = response_for_body(aws_event_frame(wrapper.as_bytes())).await;
        let (tx, _rx) = mpsc::channel(1);

        let outcome = process_aws_event_stream(response, &tx).await;

        assert!(matches!(
            outcome,
            anthropic_shared::StreamOutcome::FailedEmpty(ProviderError::Parse(_))
        ));
    }
}
