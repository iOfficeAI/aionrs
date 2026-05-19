use std::collections::HashMap;

use async_trait::async_trait;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::{RwLock, mpsc};

use aion_config::auth::{AuthConfig, OAuthManager};
use aion_config::compat::ProviderCompat;
use aion_config::debug::DebugConfig;
use aion_types::llm::{LlmEvent, LlmRequest, ThinkingConfig};

use crate::anthropic_shared::StreamOutcome;
use crate::{
    LlmProvider, ProviderError, anthropic_shared, dump_request_body, openai, reset_response_dump,
};

const MODEL_DISCOVERY_PATH: &str = "/models";
const OPENAI_INTENT: &str = "conversation-edits";
const USER_AGENT: &str = concat!("aionrs/", env!("CARGO_PKG_VERSION"));

pub struct CopilotProvider {
    client: reqwest::Client,
    api_key: String,
    base_url: String,
    auth: Option<AuthConfig>,
    oauth: Option<OAuthManager>,
    compat: ProviderCompat,
    debug: DebugConfig,
    model_routes: RwLock<Option<HashMap<String, CopilotRoute>>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CopilotRoute {
    OpenAi,
    Anthropic,
}

#[derive(Debug, Deserialize)]
struct CopilotModelsResponse {
    data: Vec<CopilotModel>,
}

#[derive(Debug, Deserialize)]
struct CopilotModel {
    model_picker_enabled: bool,
    id: String,
    #[serde(default)]
    supported_endpoints: Vec<String>,
}

struct ResolvedTransport {
    headers: HeaderMap,
    base_url: String,
    openai_api_path: String,
    messages_api_path: String,
    model_discovery_path: String,
}

impl CopilotProvider {
    pub fn new(
        provider_label: &str,
        api_key: &str,
        base_url: &str,
        compat: ProviderCompat,
        debug: DebugConfig,
        auth: Option<AuthConfig>,
    ) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_key: api_key.to_string(),
            base_url: base_url.to_string(),
            oauth: auth
                .clone()
                .filter(|_| api_key.is_empty())
                .map(|cfg| OAuthManager::new(provider_label.to_string(), cfg)),
            auth,
            compat,
            debug,
            model_routes: RwLock::new(None),
        }
    }

    fn build_headers(&self, bearer_token: &str) -> Result<HeaderMap, ProviderError> {
        let mut headers = HeaderMap::new();
        let bearer = HeaderValue::from_str(&format!("Bearer {}", bearer_token)).map_err(|e| {
            ProviderError::Connection(format!("Invalid authorization header: {}", e))
        })?;
        headers.insert(AUTHORIZATION, bearer);
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        let auth = self.auth.as_ref();
        let extra_headers = auth.map(|cfg| &cfg.api_headers);
        if let Some(extra_headers) = extra_headers {
            for (name, value) in extra_headers {
                let header_name = reqwest::header::HeaderName::from_bytes(name.as_bytes())
                    .map_err(|e| {
                        ProviderError::Connection(format!("Invalid header name: {}", e))
                    })?;
                let header_value = HeaderValue::from_str(value).map_err(|e| {
                    ProviderError::Connection(format!("Invalid header value: {}", e))
                })?;
                headers.insert(header_name, header_value);
            }
        }

        if !headers.contains_key("User-Agent") {
            headers.insert("User-Agent", HeaderValue::from_static(USER_AGENT));
        }
        if !headers.contains_key("Openai-Intent") {
            headers.insert("Openai-Intent", HeaderValue::from_static(OPENAI_INTENT));
        }
        if !headers.contains_key("x-initiator") {
            headers.insert("x-initiator", HeaderValue::from_static("agent"));
        }

        Ok(headers)
    }

    fn build_anthropic_request_body(&self, request: &LlmRequest) -> Value {
        let mut body = json!({
            "model": request.model,
            "max_tokens": request.max_tokens,
            "system": request.system,
            "messages": anthropic_shared::build_messages(&request.messages, &self.compat),
            "stream": true
        });

        if !request.tools.is_empty() {
            body["tools"] = json!(anthropic_shared::build_tools(&request.tools));
        }

        if let Some(ThinkingConfig::Enabled { budget_tokens }) = &request.thinking {
            body["thinking"] = json!({
                "type": "enabled",
                "budget_tokens": budget_tokens
            });
        }

        body
    }

    async fn resolve_transport(&self) -> Result<ResolvedTransport, ProviderError> {
        let bearer_token = if let Some(oauth) = &self.oauth {
            oauth
                .get_token()
                .await
                .map_err(|e| ProviderError::Connection(e.to_string()))?
        } else {
            self.api_key.clone()
        };

        if bearer_token.is_empty() {
            return Err(ProviderError::Connection(
                "No Copilot bearer token available. Run 'aionrs --provider copilot --login' or provide an API key.".to_string(),
            ));
        }

        let auth = self.auth.as_ref();
        Ok(ResolvedTransport {
            headers: self.build_headers(&bearer_token)?,
            base_url: auth
                .and_then(|cfg| cfg.api_base_url.clone())
                .unwrap_or_else(|| self.base_url.clone()),
            openai_api_path: auth
                .and_then(|cfg| cfg.api_path.clone())
                .unwrap_or_else(|| self.compat.api_path().to_string()),
            messages_api_path: self.compat.messages_api_path().to_string(),
            model_discovery_path: auth
                .and_then(|cfg| cfg.model_discovery_path.clone())
                .unwrap_or_else(|| MODEL_DISCOVERY_PATH.to_string()),
        })
    }

    async fn resolve_route(
        &self,
        transport: &ResolvedTransport,
        model: &str,
    ) -> Result<CopilotRoute, ProviderError> {
        if let Some(route) = self
            .model_routes
            .read()
            .await
            .as_ref()
            .and_then(|routes| routes.get(model).copied())
        {
            return Ok(route);
        }

        let url = format!("{}{}", transport.base_url, transport.model_discovery_path);
        let response = self
            .client
            .get(&url)
            .headers(transport.headers.clone())
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let body_text = response.text().await.unwrap_or_default();
            return Err(ProviderError::Api {
                status: status.as_u16(),
                message: body_text,
            });
        }

        let models: CopilotModelsResponse = response.json().await?;
        let routes: HashMap<String, CopilotRoute> = models
            .data
            .into_iter()
            .filter(|item| item.model_picker_enabled)
            .map(|item| {
                let route = if item
                    .supported_endpoints
                    .iter()
                    .any(|endpoint| endpoint == "/v1/messages")
                {
                    CopilotRoute::Anthropic
                } else {
                    CopilotRoute::OpenAi
                };
                (item.id, route)
            })
            .collect();

        let route = routes.get(model).copied().ok_or_else(|| {
            let mut available = routes.keys().cloned().collect::<Vec<_>>();
            available.sort();
            ProviderError::Connection(format!(
                "Copilot model '{}' was not returned by the discovery API. Available models: {}",
                model,
                available.join(", ")
            ))
        })?;

        *self.model_routes.write().await = Some(routes);
        Ok(route)
    }

    async fn stream_openai(
        &self,
        request: &LlmRequest,
        transport: &ResolvedTransport,
    ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        let url = format!("{}{}", transport.base_url, transport.openai_api_path);
        let body = openai::build_chat_request_body(request, &self.compat);
        dump_request_body(&self.debug, &body);
        reset_response_dump(&self.debug);

        let response = self
            .client
            .post(&url)
            .headers(transport.headers.clone())
            .json(&body)
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
            return Err(ProviderError::Api {
                status: status.as_u16(),
                message: body_text,
            });
        }

        let (tx, rx) = mpsc::channel(64);
        let debug = self.debug.clone();
        let compat = self.compat.clone();
        tokio::spawn(async move {
            if let Err(err) = openai::process_sse_stream(response, &tx, &debug, &compat).await {
                let _ = tx.send(LlmEvent::Error(err.to_string())).await;
            }
        });

        Ok(rx)
    }

    async fn stream_anthropic(
        &self,
        request: &LlmRequest,
        transport: &ResolvedTransport,
    ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        let url = format!("{}{}", transport.base_url, transport.messages_api_path);
        let body = self.build_anthropic_request_body(request);
        dump_request_body(&self.debug, &body);
        reset_response_dump(&self.debug);

        let response = self
            .client
            .post(&url)
            .headers(transport.headers.clone())
            .json(&body)
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
            return Err(ProviderError::Api {
                status: status.as_u16(),
                message: body_text,
            });
        }

        let (tx, rx) = mpsc::channel(64);
        tokio::spawn(async move {
            match anthropic_shared::process_sse_stream(response, &tx).await {
                StreamOutcome::Ok => {}
                StreamOutcome::FailedEmpty(err) | StreamOutcome::FailedPartial(err) => {
                    let _ = tx.send(LlmEvent::Error(err.to_string())).await;
                }
            }
        });

        Ok(rx)
    }
}

#[async_trait]
impl LlmProvider for CopilotProvider {
    async fn stream(
        &self,
        request: &LlmRequest,
    ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        let transport = self.resolve_transport().await?;
        match self.resolve_route(&transport, &request.model).await? {
            CopilotRoute::OpenAi => self.stream_openai(request, &transport).await,
            CopilotRoute::Anthropic => self.stream_anthropic(request, &transport).await,
        }
    }
}
