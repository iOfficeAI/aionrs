// AWS Bedrock provider for Claude models.
// Uses AWS SigV4 authentication and AWS event stream binary framing.

use async_trait::async_trait;
use aws_credential_types::Credentials;
use aws_sigv4::http_request::{
    self as sigv4_http, PayloadChecksumKind, SignableBody, SignableRequest, SignatureLocation, SigningSettings,
};
use aws_sigv4::sign::v4::SigningParams;
use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue};
use serde_json::Value;
use std::thread;
use std::time::SystemTime;
use tokio::runtime::{Handle, Runtime};
use tokio::sync::mpsc;

use aion_config::config::BedrockConfig;
use aion_types::llm::{LlmEvent, LlmRequest};

use crate::composed::ComposedProvider;
use crate::projector::{ResolvedToolWireShape, WireParams, WireProvider, classify_tools_wire_shape_mismatch};
use crate::transport::{BedrockTransport, ProjectedHttpRequest, ProviderTransport};
use crate::{LlmProvider, ProviderError};
use aion_config::compat::ProviderCompat;

pub struct BedrockProvider {
    inner: ComposedProvider,
}

impl BedrockProvider {
    pub fn new(region: &str, credentials: AwsCredentials, cache_enabled: bool, compat: ProviderCompat) -> Self {
        let transport_state = BedrockTransportState::new(region, credentials, cache_enabled);
        let transport = ProviderTransport::Bedrock(BedrockTransport {
            inner: transport_state.clone(),
        });
        let inner = ComposedProvider::new(transport, compat.clone());

        Self { inner }
    }

    #[cfg(test)]
    fn build_request_body(&self, request: &LlmRequest) -> Result<Value, ProviderError> {
        self.inner.build_request_body(request)
    }
}

#[async_trait]
impl LlmProvider for BedrockProvider {
    async fn stream(&self, request: &LlmRequest) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        self.inner.stream(request).await
    }
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

#[derive(Clone)]
pub(crate) struct BedrockTransportState {
    client: reqwest::Client,
    region: String,
    credentials: AwsCredentials,
    cache_enabled: bool,
}

impl BedrockTransportState {
    pub(crate) fn new(region: &str, credentials: AwsCredentials, cache_enabled: bool) -> Self {
        Self {
            client: reqwest::Client::new(),
            region: region.to_string(),
            credentials,
            cache_enabled,
        }
    }

    pub(crate) fn wire_params(&self, compat: &ProviderCompat) -> WireParams {
        WireParams {
            provider: WireProvider::Bedrock,
            anthropic_version: Some("bedrock-2023-05-31"),
            include_model_in_body: false,
            include_stream: false,
            cache_enabled: self.cache_enabled,
            sanitize_schema: compat.sanitize_schema(),
        }
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
        let rt = Handle::try_current();

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
                thread::scope(|s| {
                    s.spawn(|| {
                        Runtime::new()
                            .map_err(|e| ProviderError::Connection(format!("Runtime error: {}", e)))?
                            .block_on(resolve)
                    })
                    .join()
                    .unwrap()
                })
            }
            Err(_) => {
                // No runtime — safe to create one
                Runtime::new()
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
        let signing_params = SigningParams::builder()
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

        let signable_request = SignableRequest::new(method, url, header_pairs.into_iter(), SignableBody::Bytes(body))
            .map_err(|e| ProviderError::Connection(format!("Signable request error: {}", e)))?;

        let (signing_instructions, _signature) = sigv4_http::sign(signable_request, &signing_params.into())
            .map_err(|e| ProviderError::Connection(format!("SigV4 signing error: {}", e)))?
            .into_parts();

        let mut signed_headers = headers.clone();
        for (name, value) in signing_instructions.headers() {
            signed_headers.insert(
                HeaderName::from_bytes(name.as_bytes())
                    .map_err(|e| ProviderError::Connection(format!("Header name error: {}", e)))?,
                HeaderValue::from_str(value)
                    .map_err(|e| ProviderError::Connection(format!("Header value error: {}", e)))?,
            );
        }

        Ok(signed_headers)
    }

    pub(crate) fn build_projected_request(
        &self,
        model: &str,
        body: Value,
        _compat: &ProviderCompat,
        tool_wire_shape: ResolvedToolWireShape,
    ) -> Result<ProjectedHttpRequest, ProviderError> {
        let body_bytes =
            serde_json::to_vec(&body).map_err(|e| ProviderError::Connection(format!("JSON serialize error: {}", e)))?;
        let credentials = self.resolve_credentials()?;
        let url = self.build_url(model);

        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        let signed_headers = self.sign_request("POST", &url, &headers, &body_bytes, &credentials)?;

        Ok(ProjectedHttpRequest {
            url,
            headers: signed_headers,
            body,
            body_bytes: Some(body_bytes),
            tool_wire_shape,
        })
    }

    pub(crate) async fn send(&self, request: ProjectedHttpRequest) -> Result<reqwest::Response, ProviderError> {
        let ProjectedHttpRequest {
            url,
            headers,
            body_bytes,
            tool_wire_shape,
            ..
        } = request;
        let body_bytes = body_bytes.ok_or_else(|| {
            ProviderError::Connection("Bedrock projected request missing signed request body bytes".to_string())
        })?;

        let response = self.client.post(&url).headers(headers).body(body_bytes).send().await?;

        let status = response.status();
        if !status.is_success() {
            let body_text = response.text().await.unwrap_or_default();
            if status.as_u16() == 429 {
                return Err(ProviderError::RateLimited {
                    retry_after_ms: 5000,
                    body: (!body_text.is_empty()).then_some(body_text),
                });
            }
            if let Some(message) = classify_tools_wire_shape_mismatch(status.as_u16(), &body_text, tool_wire_shape) {
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

        Ok(response)
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
pub fn credentials_from_config(bc: &BedrockConfig) -> AwsCredentials {
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
#[path = "bedrock_test.rs"]
mod bedrock_test;
