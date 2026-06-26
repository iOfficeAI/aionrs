// AWS Bedrock provider for Claude models.
// Uses AWS SigV4 authentication and AWS event stream binary framing.

use async_trait::async_trait;
use aws_credential_types::Credentials;
use aws_sigv4::http_request::{
    self as sigv4_http, PayloadChecksumKind, SignableBody, SignableRequest, SignatureLocation,
    SigningSettings,
};
use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderValue};
use serde_json::Value;
use std::time::SystemTime;
use tokio::sync::mpsc;

use aion_types::llm::{LlmEvent, LlmRequest};
use aion_types::message::{StopReason, TokenUsage};

use crate::framing::bedrock_payload_to_frame;
use crate::parser::ResponseParser;
use crate::projector::{
    AnthropicWireProjector, WireParams, WireProvider, classify_tools_wire_shape_mismatch,
    projection_to_provider_error,
};
use crate::stream_runner::StreamOutcome;
use crate::{LlmProvider, ProviderError};
use aion_config::compat::ProviderCompat;

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

    fn build_request_body(&self, request: &LlmRequest) -> Result<Value, ProviderError> {
        AnthropicWireProjector::project(
            request,
            &self.compat,
            WireParams {
                provider: WireProvider::Bedrock,
                anthropic_version: Some("bedrock-2023-05-31"),
                include_model_in_body: false,
                include_stream: false,
                cache_enabled: self.cache_enabled,
                sanitize_schema: self.compat.sanitize_schema(),
            },
        )
        .map_err(projection_to_provider_error)
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
    ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        let url = self.build_url(&request.model);
        let body = self.build_request_body(request)?;
        let tool_wire_shape = AnthropicWireProjector::resolved_tool_wire_shape(&self.compat);

        tracing::debug!(target: "aion_providers", body = %serde_json::to_string_pretty(&body).unwrap_or_default(), "outgoing request");

        let body_bytes = serde_json::to_vec(&body)
            .map_err(|e| ProviderError::Connection(format!("JSON serialize error: {}", e)))?;

        let credentials = self.resolve_credentials()?;

        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        let signed_headers =
            self.sign_request("POST", &url, &headers, &body_bytes, &credentials)?;

        let response = self
            .client
            .post(&url)
            .headers(signed_headers)
            .body(body_bytes)
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let body_text = response.text().await.unwrap_or_default();
            if status.as_u16() == 429 {
                return Err(ProviderError::RateLimited {
                    retry_after_ms: 5000,
                });
            }
            if let Some(message) =
                classify_tools_wire_shape_mismatch(status.as_u16(), &body_text, tool_wire_shape)
            {
                return Err(ProviderError::Api {
                    status: status.as_u16(),
                    message,
                });
            }
            let message = format_bedrock_error(status.as_u16(), &body_text);
            return Err(ProviderError::Api {
                status: status.as_u16(),
                message,
            });
        }

        let (tx, rx) = mpsc::channel(64);

        // AWS event stream uses binary framing
        tokio::spawn(async move {
            match process_aws_event_stream(response, &tx).await {
                StreamOutcome::Ok => {}
                StreamOutcome::FailedPartial(e) | StreamOutcome::FailedEmpty(e) => {
                    // Bedrock retry requires re-signing; not implemented yet.
                    let _ = tx.send(LlmEvent::Error(e.to_string())).await;
                }
            }
        });

        Ok(rx)
    }
}

/// Process the AWS event stream (binary framed) from Bedrock
async fn process_aws_event_stream(
    response: reqwest::Response,
    tx: &mpsc::Sender<LlmEvent>,
) -> StreamOutcome {
    use futures::StreamExt;

    let parser = crate::parser::AnthropicParser;
    let mut state = parser.new_state();
    let mut buffer = Vec::new();
    let mut stream = response.bytes_stream();
    let mut emitted_content = false;
    let mut emitted_done = false;

    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(c) => c,
            Err(e) => {
                let err = ProviderError::Connection(e.to_string());
                return if emitted_content {
                    StreamOutcome::FailedPartial(err)
                } else {
                    StreamOutcome::FailedEmpty(err)
                };
            }
        };
        buffer.extend_from_slice(&chunk);

        // Parse complete AWS event stream messages from buffer
        while let Some((event_data, consumed)) = parse_aws_event(&buffer) {
            buffer = buffer[consumed..].to_vec();

            let Some(payload) = event_data else {
                continue;
            };

            if let Some(frame) = bedrock_payload_to_frame(&payload) {
                tracing::debug!(target: "aion_providers", chunk = %frame.data, "bedrock event chunk");
                let events = parser.parse_frame(&frame, &mut state);
                for event in events {
                    if matches!(
                        event,
                        LlmEvent::TextDelta(_)
                            | LlmEvent::ThinkingDelta(_)
                            | LlmEvent::ThinkingSignature(_)
                            | LlmEvent::ToolUse { .. }
                    ) {
                        emitted_content = true;
                    }
                    if matches!(event, LlmEvent::Done { .. }) {
                        emitted_done = true;
                    }
                    if tx.send(event).await.is_err() {
                        return StreamOutcome::Ok;
                    }
                }
            }
        }
    }

    // If we haven't sent a Done event, send one now
    if !emitted_done && (state.input_tokens > 0 || state.output_tokens > 0) {
        let _ = tx
            .send(LlmEvent::Done {
                stop_reason: StopReason::EndTurn,
                usage: TokenUsage {
                    input_tokens: state.input_tokens,
                    output_tokens: state.output_tokens,
                    cache_creation_tokens: state.cache_creation_tokens,
                    cache_read_tokens: state.cache_read_tokens,
                },
            })
            .await;
    }

    StreamOutcome::Ok
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
    use aion_types::message::{ContentBlock, Message, Role};
    use aion_types::tool::ToolDef;
    use base64::Engine as _;
    use serde_json::json;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    // --- Golden body snapshots (baseline for compat-split / seam-extraction refactors) ---

    fn bedrock_test_provider() -> BedrockProvider {
        BedrockProvider::new(
            "us-east-1",
            AwsCredentials::Explicit {
                access_key_id: "test-key".to_string(),
                secret_access_key: "test-secret".to_string(),
                session_token: None,
            },
            false,
            ProviderCompat::bedrock_defaults(),
        )
    }

    fn bedrock_req(messages: Vec<Message>, tools: Vec<ToolDef>) -> LlmRequest {
        LlmRequest {
            model: "test-model".to_string(),
            system: "You are a test assistant.".to_string(),
            messages,
            tools,
            max_tokens: 8192,
            thinking: None,
            reasoning_effort: None,
        }
    }

    fn bedrock_tools() -> Vec<ToolDef> {
        vec![ToolDef {
            name: "read".to_string(),
            description: "Read".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {"path": {"type": ["string", "null"]}},
                "additionalProperties": false
            }),
            deferred: false,
        }]
    }

    fn aws_event_message(payload: &[u8]) -> Vec<u8> {
        let total_len = 12 + payload.len() + 4;
        let mut message = Vec::with_capacity(total_len);
        message.extend_from_slice(&(total_len as u32).to_be_bytes());
        message.extend_from_slice(&0u32.to_be_bytes());
        message.extend_from_slice(&0u32.to_be_bytes());
        message.extend_from_slice(payload);
        message.extend_from_slice(&0u32.to_be_bytes());
        message
    }

    fn bedrock_event_payload(inner: &str) -> Vec<u8> {
        json!({
            "bytes": base64::engine::general_purpose::STANDARD.encode(inner)
        })
        .to_string()
        .into_bytes()
    }

    async fn mock_response(body: Vec<u8>) -> reqwest::Response {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/stream"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(body))
            .mount(&server)
            .await;

        reqwest::get(format!("{}/stream", server.uri()))
            .await
            .expect("mock response should be available")
    }

    async fn collect_events(mut rx: mpsc::Receiver<LlmEvent>) -> Vec<LlmEvent> {
        let mut events = Vec::new();
        while let Some(event) = rx.recv().await {
            events.push(event);
        }
        events
    }

    #[test]
    fn golden_bedrock_basic() {
        let p = bedrock_test_provider();
        let r = bedrock_req(
            vec![Message::new(
                Role::User,
                vec![ContentBlock::Text {
                    text: "Hello".to_string(),
                }],
            )],
            vec![],
        );
        insta::assert_json_snapshot!(
            "bedrock_basic",
            p.build_request_body(&r)
                .expect("request body projection should succeed")
        );
    }

    #[test]
    fn golden_bedrock_with_tools() {
        let p = bedrock_test_provider();
        let r = bedrock_req(
            vec![Message::new(
                Role::User,
                vec![ContentBlock::Text {
                    text: "go".to_string(),
                }],
            )],
            bedrock_tools(),
        );
        insta::assert_json_snapshot!(
            "bedrock_with_tools",
            p.build_request_body(&r)
                .expect("request body projection should succeed")
        );
    }

    #[tokio::test]
    async fn bedrock_event_stream_decodes_payloads_into_llm_events() {
        let mut body = Vec::new();
        for inner in [
            r#"{"type":"message_start","message":{"usage":{"input_tokens":12}}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}"#,
            r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":7}}"#,
        ] {
            body.extend(aws_event_message(&bedrock_event_payload(inner)));
        }

        let response = mock_response(body).await;
        let (tx, rx) = mpsc::channel(8);

        let outcome = process_aws_event_stream(response, &tx).await;
        drop(tx);
        let events = collect_events(rx).await;

        assert!(matches!(outcome, StreamOutcome::Ok));
        assert_eq!(events.len(), 2);
        assert!(matches!(&events[0], LlmEvent::TextDelta(text) if text == "Hello"));
        match &events[1] {
            LlmEvent::Done { stop_reason, usage } => {
                assert_eq!(*stop_reason, StopReason::EndTurn);
                assert_eq!(usage.input_tokens, 12);
                assert_eq!(usage.output_tokens, 7);
            }
            event => panic!("expected Done event, got {event:?}"),
        }
    }

    #[tokio::test]
    async fn bedrock_event_stream_synthesizes_done_when_message_delta_is_missing() {
        let mut body = Vec::new();
        for inner in [
            r#"{"type":"message_start","message":{"usage":{"input_tokens":12}}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}"#,
        ] {
            body.extend(aws_event_message(&bedrock_event_payload(inner)));
        }

        let response = mock_response(body).await;
        let (tx, rx) = mpsc::channel(8);

        let outcome = process_aws_event_stream(response, &tx).await;
        drop(tx);
        let events = collect_events(rx).await;

        assert!(matches!(outcome, StreamOutcome::Ok));
        assert_eq!(events.len(), 2);
        assert!(matches!(&events[0], LlmEvent::TextDelta(text) if text == "Hello"));
        match &events[1] {
            LlmEvent::Done { stop_reason, usage } => {
                assert_eq!(*stop_reason, StopReason::EndTurn);
                assert_eq!(usage.input_tokens, 12);
                assert_eq!(usage.output_tokens, 0);
            }
            event => panic!("expected synthesized Done event, got {event:?}"),
        }
    }
}
