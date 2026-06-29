// Google Vertex AI provider for Claude models.
// Uses GCP OAuth2 authentication. Response is standard SSE (same as Anthropic).

use async_trait::async_trait;
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs::read_to_string;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;

use aion_config::config::VertexConfig;
use aion_types::llm::{LlmEvent, LlmRequest};

use crate::composed::ComposedProvider;
use crate::projector::{ResolvedToolWireShape, WireParams, WireProvider, classify_tools_wire_shape_mismatch};
use crate::transport::{ProjectedHttpRequest, ProviderTransport, VertexTransport};
use crate::{LlmProvider, ProviderError};
use aion_config::compat::ProviderCompat;

#[derive(Clone)]
pub struct VertexProvider {
    inner: ComposedProvider,
}

impl VertexProvider {
    pub fn new(project_id: &str, region: &str, auth: GcpAuth, cache_enabled: bool, compat: ProviderCompat) -> Self {
        let transport_state = VertexTransportState::new(project_id, region, auth, cache_enabled);
        let transport = ProviderTransport::Vertex(VertexTransport {
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
impl LlmProvider for VertexProvider {
    async fn stream(&self, request: &LlmRequest) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        self.inner.stream(request).await
    }
}

#[derive(Debug, Clone)]
pub enum GcpAuth {
    ServiceAccount { key_file: String },
    ApplicationDefault,
    MetadataServer,
}

#[derive(Clone)]
pub(crate) struct VertexTransportState {
    client: reqwest::Client,
    project_id: String,
    region: String,
    auth: GcpAuth,
    cache_enabled: bool,
    /// Cached access token
    cached_token: Arc<Mutex<Option<CachedToken>>>,
}

impl VertexTransportState {
    pub(crate) fn new(project_id: &str, region: &str, auth: GcpAuth, cache_enabled: bool) -> Self {
        Self {
            client: reqwest::Client::new(),
            project_id: project_id.to_string(),
            region: region.to_string(),
            auth,
            cache_enabled,
            cached_token: Arc::new(Mutex::new(None)),
        }
    }

    pub(crate) fn wire_params(&self) -> WireParams {
        WireParams {
            provider: WireProvider::Vertex,
            anthropic_version: Some("vertex-2023-10-16"),
            include_model_in_body: false,
            include_stream: true,
            cache_enabled: self.cache_enabled,
            sanitize_schema: false,
        }
    }

    fn build_url(&self, model: &str) -> String {
        format!(
            "https://{}-aiplatform.googleapis.com/v1/projects/{}/locations/{}/publishers/anthropic/models/{}:streamRawPredict",
            self.region, self.project_id, self.region, model
        )
    }

    pub(crate) fn build_projected_request(
        &self,
        model: &str,
        body: Value,
        _compat: &ProviderCompat,
        tool_wire_shape: ResolvedToolWireShape,
    ) -> Result<ProjectedHttpRequest, ProviderError> {
        Ok(ProjectedHttpRequest {
            url: self.build_url(model),
            headers: HeaderMap::new(),
            body,
            body_bytes: None,
            tool_wire_shape,
        })
    }

    async fn get_access_token(&self) -> Result<String, ProviderError> {
        // Check cache first
        {
            let cached = self
                .cached_token
                .lock()
                .map_err(|_| ProviderError::Connection("Vertex token cache lock poisoned".to_string()))?;
            if let Some(token) = cached.as_ref() {
                let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
                if token.expires_at > now + 60 {
                    return Ok(token.token.clone());
                }
            }
        }

        let (token, expires_in) = match &self.auth {
            GcpAuth::ServiceAccount { key_file } => self.get_service_account_token(key_file).await?,
            GcpAuth::ApplicationDefault => self.get_adc_token().await?,
            GcpAuth::MetadataServer => self.get_metadata_token().await?,
        };

        // Cache the token
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        let mut cached = self
            .cached_token
            .lock()
            .map_err(|_| ProviderError::Connection("Vertex token cache lock poisoned".to_string()))?;
        *cached = Some(CachedToken {
            token: token.clone(),
            expires_at: now + expires_in,
        });

        Ok(token)
    }

    async fn get_service_account_token(&self, key_file: &str) -> Result<(String, u64), ProviderError> {
        let key_json = read_to_string(key_file)
            .map_err(|e| ProviderError::Connection(format!("Failed to read key file: {}", e)))?;

        let sa: ServiceAccountKey = serde_json::from_str(&key_json)
            .map_err(|e| ProviderError::Connection(format!("Failed to parse key file: {}", e)))?;

        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();

        let claims = JwtClaims {
            iss: sa.client_email.clone(),
            scope: "https://www.googleapis.com/auth/cloud-platform".to_string(),
            aud: sa.token_uri.clone(),
            iat: now,
            exp: now + 3600,
        };

        let encoding_key = EncodingKey::from_rsa_pem(sa.private_key.as_bytes())
            .map_err(|e| ProviderError::Connection(format!("Invalid RSA key: {}", e)))?;

        let header = Header::new(Algorithm::RS256);
        let jwt = jsonwebtoken::encode(&header, &claims, &encoding_key)
            .map_err(|e| ProviderError::Connection(format!("JWT encode error: {}", e)))?;

        // Exchange JWT for access token
        let resp = self
            .client
            .post(&sa.token_uri)
            .form(&[
                ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
                ("assertion", &jwt),
            ])
            .send()
            .await
            .map_err(|e| ProviderError::Connection(format!("Token exchange error: {}", e)))?;

        let token_resp: GoogleTokenResponse = resp
            .json()
            .await
            .map_err(|e| ProviderError::Connection(format!("Token parse error: {}", e)))?;

        Ok((token_resp.access_token, token_resp.expires_in))
    }

    async fn get_adc_token(&self) -> Result<(String, u64), ProviderError> {
        // Read Application Default Credentials
        let adc_path = dirs::home_dir()
            .ok_or_else(|| ProviderError::Connection("Cannot determine home dir".into()))?
            .join(".config/gcloud/application_default_credentials.json");

        let adc_json = read_to_string(&adc_path).map_err(|e| {
            ProviderError::Connection(format!(
                "Failed to read ADC at {}: {}. Run 'gcloud auth application-default login'.",
                adc_path.display(),
                e
            ))
        })?;

        let adc: AdcCredentials = serde_json::from_str(&adc_json)
            .map_err(|e| ProviderError::Connection(format!("Failed to parse ADC: {}", e)))?;

        // Use refresh token to get access token
        let resp = self
            .client
            .post("https://oauth2.googleapis.com/token")
            .form(&[
                ("client_id", adc.client_id.as_str()),
                ("client_secret", adc.client_secret.as_str()),
                ("refresh_token", adc.refresh_token.as_str()),
                ("grant_type", "refresh_token"),
            ])
            .send()
            .await
            .map_err(|e| ProviderError::Connection(format!("ADC token refresh error: {}", e)))?;

        let token_resp: GoogleTokenResponse = resp
            .json()
            .await
            .map_err(|e| ProviderError::Connection(format!("Token parse error: {}", e)))?;

        Ok((token_resp.access_token, token_resp.expires_in))
    }

    async fn get_metadata_token(&self) -> Result<(String, u64), ProviderError> {
        let resp = self
            .client
            .get("http://metadata.google.internal/computeMetadata/v1/instance/service-accounts/default/token")
            .header("Metadata-Flavor", "Google")
            .send()
            .await
            .map_err(|e| ProviderError::Connection(format!("Metadata server error: {}", e)))?;

        let token_resp: GoogleTokenResponse = resp
            .json()
            .await
            .map_err(|e| ProviderError::Connection(format!("Token parse error: {}", e)))?;

        Ok((token_resp.access_token, token_resp.expires_in))
    }

    pub(crate) async fn send(&self, request: ProjectedHttpRequest) -> Result<reqwest::Response, ProviderError> {
        let ProjectedHttpRequest {
            url,
            body,
            tool_wire_shape,
            ..
        } = request;

        self.send_stream_request(&url, &body, tool_wire_shape).await
    }

    async fn send_stream_request(
        &self,
        url: &str,
        body: &Value,
        tool_wire_shape: ResolvedToolWireShape,
    ) -> Result<reqwest::Response, ProviderError> {
        let access_token = self.get_access_token().await?;

        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", access_token))
                .map_err(|e| ProviderError::Connection(format!("Header error: {}", e)))?,
        );

        let response = self.client.post(url).headers(headers).json(body).send().await?;

        let status = response.status();
        if !status.is_success() {
            let body_text = response.text().await.unwrap_or_default();
            if status.as_u16() == 429 {
                return Err(ProviderError::RateLimited { retry_after_ms: 5000 });
            }
            if let Some(message) = classify_tools_wire_shape_mismatch(status.as_u16(), &body_text, tool_wire_shape) {
                return Err(ProviderError::Api {
                    status: status.as_u16(),
                    message,
                });
            }
            return Err(ProviderError::Api {
                status: status.as_u16(),
                message: body_text,
            });
        }

        Ok(response)
    }
}

struct CachedToken {
    token: String,
    expires_at: u64,
}

// --- Internal types ---

#[derive(Debug, Deserialize)]
struct ServiceAccountKey {
    client_email: String,
    private_key: String,
    token_uri: String,
}

#[derive(Debug, Serialize)]
struct JwtClaims {
    iss: String,
    scope: String,
    aud: String,
    iat: u64,
    exp: u64,
}

#[derive(Debug, Deserialize)]
struct GoogleTokenResponse {
    access_token: String,
    #[serde(default = "default_expires_in")]
    expires_in: u64,
}

fn default_expires_in() -> u64 {
    3600
}

#[derive(Debug, Deserialize)]
struct AdcCredentials {
    client_id: String,
    client_secret: String,
    refresh_token: String,
}

/// Build GcpAuth from aion-config's VertexConfig
pub fn auth_from_config(vc: &VertexConfig) -> GcpAuth {
    if let Some(creds_file) = &vc.credentials_file {
        GcpAuth::ServiceAccount {
            key_file: creds_file.clone(),
        }
    } else {
        GcpAuth::ApplicationDefault
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aion_config::compat::ProviderCompat;
    use aion_types::message::{ContentBlock, Message, Role};
    use aion_types::tool::ToolDef;
    use serde_json::json;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use crate::projector::ResolvedToolWireShape;
    use crate::transport::{ProjectedHttpRequest, ProviderTransport, VertexTransport};

    // --- Golden body snapshots (baseline for compat-split / seam-extraction refactors) ---

    fn vertex_test_provider() -> VertexProvider {
        VertexProvider::new(
            "test-project",
            "us-central1",
            GcpAuth::ApplicationDefault,
            false,
            ProviderCompat::anthropic_defaults(),
        )
    }

    fn vertex_req(messages: Vec<Message>, tools: Vec<ToolDef>) -> LlmRequest {
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

    #[test]
    fn vertex_provider_preserves_clone_api() {
        fn assert_clone<T: Clone>() {}

        assert_clone::<VertexProvider>();
    }

    #[test]
    fn golden_vertex_basic() {
        let p = vertex_test_provider();
        let r = vertex_req(
            vec![Message::new(
                Role::User,
                vec![ContentBlock::Text {
                    text: "Hello".to_string(),
                }],
            )],
            vec![],
        );
        insta::assert_json_snapshot!(
            "vertex_basic",
            p.build_request_body(&r)
                .expect("request body projection should succeed")
        );
    }

    #[test]
    fn vertex_transport_builds_projected_request_with_vertex_url_and_preserves_body() {
        let state = VertexTransportState::new("test-project", "us-central1", GcpAuth::ApplicationDefault, false);
        let transport = ProviderTransport::Vertex(VertexTransport { inner: state });
        let compat = ProviderCompat::anthropic_defaults();
        let body = json!({
            "anthropic_version": "vertex-2023-10-16",
            "messages": [{"role": "user", "content": "Hello"}],
            "stream": true
        });
        let tool_wire_shape = ResolvedToolWireShape::AnthropicInputSchema;

        let request = transport
            .build_projected_request("claude-test-model", body.clone(), &compat, tool_wire_shape)
            .expect("vertex projected request should build");

        assert_eq!(
            request.url,
            "https://us-central1-aiplatform.googleapis.com/v1/projects/test-project/locations/us-central1/publishers/anthropic/models/claude-test-model:streamRawPredict"
        );
        assert!(request.headers.is_empty());
        assert_eq!(request.body, body);
        assert!(request.body_bytes.is_none());
        assert_eq!(request.tool_wire_shape, tool_wire_shape);
    }

    #[tokio::test]
    async fn vertex_transport_send_maps_tool_shape_mismatch_to_actionable_api_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/streamRawPredict"))
            .and(header("authorization", "Bearer cached-token"))
            .and(header("content-type", "application/json"))
            .respond_with(
                ResponseTemplate::new(400).set_body_string("invalid_request_error: body.tools[0].function is missing"),
            )
            .mount(&server)
            .await;

        let state = VertexTransportState::new("test-project", "us-central1", GcpAuth::ApplicationDefault, false);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be after unix epoch")
            .as_secs();
        *state
            .cached_token
            .lock()
            .expect("token cache lock should not be poisoned") = Some(CachedToken {
            token: "cached-token".to_string(),
            expires_at: now + 3600,
        });
        let transport = ProviderTransport::Vertex(VertexTransport { inner: state });
        let request = ProjectedHttpRequest {
            url: format!("{}/streamRawPredict", server.uri()),
            headers: HeaderMap::new(),
            body: json!({"messages": []}),
            body_bytes: None,
            tool_wire_shape: ResolvedToolWireShape::AnthropicInputSchema,
        };

        let error = transport
            .send(request)
            .await
            .expect_err("tool shape mismatch should map to api error");

        assert!(matches!(
            error,
            ProviderError::Api { status: 400, message }
                if message.contains("tools wire shape mismatch")
                    && message.contains("anthropic_input_schema")
                    && message.contains("openai_function")
        ));
    }
}
