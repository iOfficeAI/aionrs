use async_trait::async_trait;
use rand::RngCore;
use reqwest::header::{
    AUTHORIZATION, CONNECTION, CONTENT_TYPE, HeaderMap, HeaderValue, RETRY_AFTER, USER_AGENT,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::LazyLock;
use tokio::sync::mpsc;

use aion_config::auth::{AuthConfig, OAuthManager, build_auth_http_client};
use aion_types::llm::{
    AccountCreditsInfo, AccountLimitInfo, AccountLimitWindow, AccountLimitsInfo, LlmEvent,
    LlmRequest, ProviderMetadata, ProviderModelInfo,
};
use aion_types::message::{ContentBlock, Message, Role, StopReason, TokenUsage};
use aion_types::tool::{ToolDef, truncate_deferred_description};

use crate::retry::{retry_delay, with_retry_if_notify_budget};
use crate::{
    LlmProvider, ProviderError, RetryAttempt, RetryObserver, dump_request_body,
    dump_response_chunk, reset_response_dump,
};
use aion_config::compat::ProviderCompat;
use aion_config::debug::DebugConfig;

pub struct OpenAIProvider {
    client: reqwest::Client,
    api_key: String,
    base_url: String,
    auth: Option<AuthConfig>,
    oauth: Option<OAuthManager>,
    compat: ProviderCompat,
    debug: DebugConfig,
}

const OPENAI_USER_AGENT: &str = concat!("aionrs/", env!("CARGO_PKG_VERSION"));
const CHATGPT_DEFAULT_MODEL_DISCOVERY_PATH: &str = "/backend-api/codex/models";
const CHATGPT_DEFAULT_USAGE_PATH: &str = "/backend-api/wham/usage";
const CHATGPT_CLIENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const STREAM_REQUEST_CONNECTION_RETRIES: u32 = 4;
const STREAM_REQUEST_RATE_LIMIT_RETRIES: u32 = 12;
const STREAM_IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(300_000);
const SYNTHETIC_TOOL_CALL_REASONING_CONTENT: &str =
    "The assistant selected tool calls; reasoning content was not provided by the upstream.";
static CHATGPT_INSTALLATION_ID: LazyLock<String> = LazyLock::new(random_uuid_v4_string);

impl OpenAIProvider {
    pub fn new(
        provider_label: &str,
        api_key: &str,
        base_url: &str,
        compat: ProviderCompat,
        debug: DebugConfig,
        auth: Option<AuthConfig>,
    ) -> Self {
        let client = auth
            .as_ref()
            .map(build_auth_http_client)
            .unwrap_or_default();

        Self {
            client,
            api_key: api_key.to_string(),
            base_url: base_url.to_string(),
            oauth: auth
                .clone()
                .filter(|_| api_key.is_empty())
                .map(|cfg| OAuthManager::new(provider_label.to_string(), cfg)),
            auth,
            compat,
            debug,
        }
    }

    fn build_headers(
        &self,
        bearer_token: &str,
        auth: Option<&AuthConfig>,
        account_header: Option<&str>,
        account_id: Option<&str>,
    ) -> Result<HeaderMap, ProviderError> {
        let mut headers = HeaderMap::new();
        let bearer = format!("Bearer {}", bearer_token);
        let auth_header = HeaderValue::from_str(&bearer).map_err(|e| {
            ProviderError::Connection(format!("Invalid authorization header: {}", e))
        })?;
        headers.insert(AUTHORIZATION, auth_header);
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        if let Some(auth) = auth {
            for (name, value) in &auth.api_headers {
                let header_name = reqwest::header::HeaderName::from_bytes(name.as_bytes())
                    .map_err(|e| {
                        ProviderError::Connection(format!("Invalid header name: {}", e))
                    })?;
                let header_value = HeaderValue::from_str(value).map_err(|e| {
                    ProviderError::Connection(format!("Invalid header value: {}", e))
                })?;
                headers.insert(header_name, header_value);
            }
            if auth.disable_connection_reuse {
                headers.insert(CONNECTION, HeaderValue::from_static("close"));
            }
        }
        if let (Some(header_name), Some(account_id)) = (account_header, account_id) {
            let header_name = reqwest::header::HeaderName::from_bytes(header_name.as_bytes())
                .map_err(|e| ProviderError::Connection(format!("Invalid account header: {}", e)))?;
            let value = HeaderValue::from_str(account_id).map_err(|e| {
                ProviderError::Connection(format!("Invalid account id header: {}", e))
            })?;
            headers.insert(header_name, value);
        }
        if !headers.contains_key(USER_AGENT) {
            headers.insert(USER_AGENT, HeaderValue::from_static(OPENAI_USER_AGENT));
        }
        Ok(headers)
    }

    pub(crate) fn build_messages(
        messages: &[Message],
        system: &str,
        compat: &ProviderCompat,
    ) -> Vec<Value> {
        let mut result: Vec<Value> = Vec::new();

        // System message first
        if !system.is_empty() {
            result.push(json!({
                "role": "system",
                "content": system
            }));
        }

        for msg in messages {
            match msg.role {
                Role::User => {
                    // Check if this contains tool results
                    let has_tool_results = msg
                        .content
                        .iter()
                        .any(|b| matches!(b, ContentBlock::ToolResult { .. }));

                    if has_tool_results {
                        // Each tool result becomes a separate "tool" role message
                        for block in &msg.content {
                            if let ContentBlock::ToolResult {
                                tool_use_id,
                                content,
                                ..
                            } = block
                                && !tool_use_id.is_empty()
                            {
                                result.push(json!({
                                    "role": "tool",
                                    "tool_call_id": tool_use_id,
                                    "content": content
                                }));
                            }
                        }
                    } else {
                        let text: String = msg
                            .content
                            .iter()
                            .filter_map(|b| {
                                if let ContentBlock::Text { text } = b {
                                    Some(text.as_str())
                                } else {
                                    None
                                }
                            })
                            .collect::<Vec<_>>()
                            .join("\n");
                        let text = strip_patterns_from_text(&text, compat);
                        result.push(json!({
                            "role": "user",
                            "content": text
                        }));
                    }
                }
                Role::Assistant => {
                    let mut msg_json = json!({ "role": "assistant" });

                    // Preserve reasoning_content for models with thinking mode
                    // (e.g. DeepSeek Reasoner, Kimi K2.5). The API requires
                    // previous reasoning_content to be sent back in multi-turn.
                    let thinking: String = msg
                        .content
                        .iter()
                        .filter_map(|b| {
                            if let ContentBlock::Thinking { thinking } = b {
                                Some(thinking.as_str())
                            } else {
                                None
                            }
                        })
                        .collect::<Vec<_>>()
                        .join("");

                    if !thinking.is_empty() {
                        msg_json["reasoning_content"] = json!(thinking);
                    }

                    let text: String = msg
                        .content
                        .iter()
                        .filter_map(|b| {
                            if let ContentBlock::Text { text } = b {
                                Some(text.as_str())
                            } else {
                                None
                            }
                        })
                        .collect::<Vec<_>>()
                        .join("");
                    let text = strip_patterns_from_text(&text, compat);
                    let text = strip_assistant_text_sentinels(&text, compat);

                    let tool_calls: Vec<Value> = msg
                        .content
                        .iter()
                        .filter_map(|b| {
                            if let ContentBlock::ToolUse {
                                id,
                                name,
                                input,
                                extra,
                            } = b
                                && !id.is_empty()
                            {
                                let mut tool_call = json!({
                                    "id": id,
                                    "type": "function",
                                    "function": {
                                        "name": name,
                                        "arguments": serde_json::to_string(input).unwrap_or_default()
                                    }
                                });
                                if let Some(extra) = extra {
                                    tool_call["extra_content"] = extra.clone();
                                }
                                Some(tool_call)
                            } else {
                                None
                            }
                        })
                        .collect();

                    msg_json["content"] = json!(text);

                    if !tool_calls.is_empty() {
                        if thinking.is_empty()
                            && compat.synthesize_missing_tool_call_reasoning_content()
                        {
                            msg_json["reasoning_content"] =
                                json!(SYNTHETIC_TOOL_CALL_REASONING_CONTENT);
                        }
                        msg_json["tool_calls"] = json!(tool_calls);
                    }

                    result.push(msg_json);
                }
                Role::System => {
                    // Already handled above
                }
                Role::Tool => {
                    for block in &msg.content {
                        if let ContentBlock::ToolResult {
                            tool_use_id,
                            content,
                            ..
                        } = block
                            && !tool_use_id.is_empty()
                        {
                            result.push(json!({
                                "role": "tool",
                                "tool_call_id": tool_use_id,
                                "content": content
                            }));
                        }
                    }
                }
            }
        }

        // Dedup tool results: keep last occurrence of each tool_call_id
        if compat.dedup_tool_results() {
            dedup_tool_results(&mut result);
        }

        // Clean orphan tool calls: remove tool_call entries with no matching tool result
        if compat.clean_orphan_tool_calls() {
            clean_orphaned_tool_calls(&mut result);
        }

        // Merge consecutive assistant messages
        if compat.merge_assistant_messages() {
            merge_consecutive_assistant(&mut result);
        }

        result
    }

    pub(crate) fn build_tools(tools: &[ToolDef]) -> Vec<Value> {
        tools
            .iter()
            .map(|t| {
                if t.deferred {
                    let short_desc = truncate_deferred_description(&t.description);
                    json!({
                        "type": "function",
                        "function": {
                            "name": t.name,
                            "description": format!(
                                "(Deferred) {short_desc} — Use ToolSearch to load full schema before calling."
                            ),
                            "parameters": {
                                "type": "object",
                                "properties": {}
                            }
                        }
                    })
                } else {
                    json!({
                        "type": "function",
                        "function": {
                            "name": t.name,
                            "description": t.description,
                            "parameters": t.input_schema
                        }
                    })
                }
            })
            .collect()
    }

    fn build_responses_tools(tools: &[ToolDef]) -> Vec<Value> {
        tools
            .iter()
            .map(|t| {
                json!({
                    "type": "function",
                    "name": t.name,
                    "description": t.description,
                    "parameters": t.input_schema
                })
            })
            .collect()
    }

    fn build_responses_input(messages: &[Value]) -> Vec<Value> {
        let mut input = Vec::new();

        for message in messages {
            let Some(role) = message["role"].as_str() else {
                continue;
            };

            match role {
                "system" => {
                    if let Some(content) = message["content"].as_str() {
                        input.push(json!({
                            "role": "system",
                            "content": content,
                        }));
                    }
                }
                "user" => {
                    if let Some(content) = message["content"].as_str() {
                        input.push(json!({
                            "role": "user",
                            "content": [{ "type": "input_text", "text": content }],
                        }));
                    }
                }
                "assistant" => {
                    if let Some(content) = message["content"]
                        .as_str()
                        .filter(|value| !value.is_empty())
                    {
                        input.push(json!({
                            "role": "assistant",
                            "content": [{ "type": "output_text", "text": content }],
                        }));
                    }
                    if let Some(tool_calls) = message["tool_calls"].as_array() {
                        for tool_call in tool_calls {
                            let Some(call_id) =
                                tool_call["id"].as_str().filter(|value| !value.is_empty())
                            else {
                                continue;
                            };
                            let name = tool_call["function"]["name"].as_str().unwrap_or_default();
                            let arguments = tool_call["function"]["arguments"]
                                .as_str()
                                .unwrap_or_default();
                            input.push(json!({
                                "type": "function_call",
                                "call_id": call_id,
                                "name": name,
                                "arguments": arguments,
                            }));
                        }
                    }
                }
                "tool" => {
                    let Some(call_id) = message["tool_call_id"]
                        .as_str()
                        .filter(|value| !value.is_empty())
                    else {
                        continue;
                    };
                    let content = message["content"].as_str().unwrap_or_default().to_string();
                    input.push(json!({
                        "type": "function_call_output",
                        "call_id": call_id,
                        "output": content,
                    }));
                }
                _ => {}
            }
        }

        input
    }

    fn build_request_body(&self, request: &LlmRequest) -> Value {
        build_chat_request_body(request, &self.compat)
    }

    fn build_responses_request_body(
        &self,
        request: &LlmRequest,
        system_as_instructions: bool,
        include_max_output_tokens: bool,
    ) -> Value {
        let session_id = self
            .compat
            .codex_session_identity()
            .then(|| normalized_session_id(request.session_id.as_deref()))
            .flatten();
        let system = if system_as_instructions {
            ""
        } else {
            &request.system
        };
        let messages = Self::build_messages(&request.messages, system, &self.compat);
        let mut body = json!({
            "model": request.model,
            "input": Self::build_responses_input(&messages),
            "stream": true,
            "store": false,
        });

        if include_max_output_tokens {
            body["max_output_tokens"] = json!(request.max_tokens);
        }

        if system_as_instructions && !request.system.is_empty() {
            body["instructions"] = json!(request.system);
        }

        if !request.tools.is_empty() {
            body["tools"] = json!(Self::build_responses_tools(&request.tools));
        }

        if let Some(effort) = &request.reasoning_effort {
            body["reasoning"] = json!({ "effort": effort });
        }

        if let Some(session_id) = session_id {
            body["prompt_cache_key"] = json!(session_id);
            body["client_metadata"] = json!({
                "x-codex-installation-id": CHATGPT_INSTALLATION_ID.as_str(),
            });
        }

        body
    }

    async fn resolve_transport(&self) -> Result<ResolvedTransport, ProviderError> {
        if let Some(oauth) = &self.oauth {
            let credentials = oauth
                .get_credentials()
                .await
                .map_err(|e| ProviderError::Connection(e.to_string()))?;
            let auth = self.auth.as_ref();
            return Ok(ResolvedTransport {
                include_max_output_tokens: auth.and_then(|cfg| cfg.auth_mode.as_deref())
                    != Some("chatgpt"),
                headers: self.build_headers(
                    &credentials.tokens.access_token,
                    auth,
                    auth.and_then(|cfg| cfg.account_id_header.as_deref()),
                    credentials.tokens.account_id.as_deref(),
                )?,
                base_url: auth
                    .and_then(|cfg| cfg.api_base_url.clone())
                    .unwrap_or_else(|| self.base_url.clone()),
                api_path: auth
                    .and_then(|cfg| cfg.api_path.clone())
                    .unwrap_or_else(|| self.compat.api_path().to_string()),
                use_responses_api: auth.map(|cfg| cfg.use_responses_api).unwrap_or(false),
                system_as_instructions: auth.map(|cfg| cfg.system_as_instructions).unwrap_or(false),
                model_discovery_path: auth
                    .and_then(|cfg| cfg.model_discovery_path.clone())
                    .or_else(|| {
                        auth.filter(|cfg| cfg.use_responses_api)
                            .map(|_| CHATGPT_DEFAULT_MODEL_DISCOVERY_PATH.to_string())
                    }),
                usage_path: auth.and_then(|cfg| cfg.usage_path.clone()).or_else(|| {
                    auth.filter(|cfg| cfg.use_responses_api)
                        .map(|_| CHATGPT_DEFAULT_USAGE_PATH.to_string())
                }),
            });
        }

        Ok(ResolvedTransport {
            headers: self.build_headers(&self.api_key, None, None, None)?,
            base_url: self.base_url.clone(),
            api_path: self.compat.api_path().to_string(),
            use_responses_api: false,
            system_as_instructions: false,
            include_max_output_tokens: true,
            model_discovery_path: None,
            usage_path: None,
        })
    }

    async fn fetch_models(
        &self,
        transport: &ResolvedTransport,
    ) -> Result<Vec<ProviderModelInfo>, ProviderError> {
        let Some(model_discovery_path) = transport.model_discovery_path.as_deref() else {
            return Ok(Vec::new());
        };

        let mut url = format!("{}{}", transport.base_url, model_discovery_path);
        if transport.use_responses_api {
            url = append_query_param(&url, "client_version", CHATGPT_CLIENT_VERSION);
        }
        let response = self
            .client
            .get(&url)
            .headers(transport.headers.clone())
            .send()
            .await
            .map_err(|err| request_send_error("GET", &url, err))?;
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(metadata_status_error(status.as_u16(), body));
        }

        parse_provider_models_response(&body)
    }

    async fn fetch_account_limits(
        &self,
        transport: &ResolvedTransport,
    ) -> Result<Option<AccountLimitsInfo>, ProviderError> {
        let Some(usage_path) = transport.usage_path.as_deref() else {
            return Ok(None);
        };

        let url = format!("{}{}", transport.base_url, usage_path);
        let response = self
            .client
            .get(&url)
            .headers(transport.headers.clone())
            .send()
            .await
            .map_err(|err| request_send_error("GET", &url, err))?;
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(metadata_status_error(status.as_u16(), body));
        }

        parse_account_limits_response(&body)
    }

    async fn refresh_after_unauthorized(&self) -> Result<(), ProviderError> {
        let Some(oauth) = &self.oauth else {
            return Err(ProviderError::Api {
                status: 401,
                message: "Unauthorized".to_string(),
            });
        };

        oauth
            .refresh_credentials_after_unauthorized()
            .await
            .map(|_| ())
            .map_err(|err| ProviderError::Connection(err.to_string()))
    }
}

struct ResolvedTransport {
    headers: HeaderMap,
    base_url: String,
    api_path: String,
    use_responses_api: bool,
    system_as_instructions: bool,
    include_max_output_tokens: bool,
    model_discovery_path: Option<String>,
    usage_path: Option<String>,
}

fn normalized_session_id(session_id: Option<&str>) -> Option<&str> {
    session_id.map(str::trim).filter(|value| !value.is_empty())
}

fn apply_responses_request_identity(
    headers: &mut HeaderMap,
    session_id: Option<&str>,
    enabled: bool,
) -> Result<(), ProviderError> {
    if !enabled {
        return Ok(());
    }
    let Some(session_id) = normalized_session_id(session_id) else {
        return Ok(());
    };

    insert_header_value(headers, "session_id", session_id)?;
    insert_header_value(headers, "x-client-request-id", session_id)?;
    insert_header_value(headers, "x-codex-window-id", &format!("{session_id}:0"))?;
    insert_header_value(headers, "version", CHATGPT_CLIENT_VERSION)?;
    Ok(())
}

fn insert_header_value(
    headers: &mut HeaderMap,
    name: &'static str,
    value: &str,
) -> Result<(), ProviderError> {
    let header_value = HeaderValue::from_str(value)
        .map_err(|e| ProviderError::Connection(format!("Invalid {name} header: {e}")))?;
    headers.insert(name, header_value);
    Ok(())
}

fn random_uuid_v4_string() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15]
    )
}

#[derive(Debug, Deserialize)]
struct ChatGptModelsResponse {
    #[serde(default)]
    models: Vec<ChatGptModelInfo>,
}

#[derive(Debug, Deserialize)]
struct ChatGptModelInfo {
    slug: String,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    default_reasoning_level: Option<String>,
    #[serde(default)]
    supported_reasoning_levels: Option<Vec<ChatGptReasoningLevel>>,
    #[serde(default)]
    context_window: Option<u64>,
    #[serde(default)]
    visibility: Option<String>,
    #[serde(default)]
    priority: Option<i32>,
}

#[derive(Debug, Deserialize)]
struct ChatGptReasoningLevel {
    effort: String,
}

#[derive(Debug, Deserialize)]
struct ChatGptUsageResponse {
    #[serde(default)]
    plan_type: Option<String>,
    #[serde(default)]
    rate_limit: Option<ChatGptRateLimitDetails>,
    #[serde(default)]
    additional_rate_limits: Option<Vec<ChatGptAdditionalRateLimitDetails>>,
    #[serde(default)]
    credits: Option<ChatGptCreditDetails>,
}

#[derive(Debug, Deserialize)]
struct ChatGptAdditionalRateLimitDetails {
    limit_name: String,
    metered_feature: String,
    #[serde(default)]
    rate_limit: Option<ChatGptRateLimitDetails>,
}

#[derive(Debug, Deserialize)]
struct ChatGptRateLimitDetails {
    #[serde(default)]
    primary_window: Option<ChatGptRateLimitWindow>,
    #[serde(default)]
    secondary_window: Option<ChatGptRateLimitWindow>,
}

#[derive(Debug, Deserialize)]
struct ChatGptRateLimitWindow {
    used_percent: f64,
    limit_window_seconds: u64,
    #[serde(default)]
    reset_at: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct ChatGptCreditDetails {
    has_credits: bool,
    unlimited: bool,
    #[serde(default)]
    balance: Option<String>,
}

fn metadata_status_error(status: u16, body: String) -> ProviderError {
    if status == 429 {
        ProviderError::RateLimited {
            retry_after_ms: 5000,
        }
    } else {
        ProviderError::Api {
            status,
            message: body,
        }
    }
}

fn parse_provider_models_response(body: &str) -> Result<Vec<ProviderModelInfo>, ProviderError> {
    let mut response: ChatGptModelsResponse =
        serde_json::from_str(body).map_err(|e| ProviderError::Parse(e.to_string()))?;

    response.models.sort_by(|left, right| {
        left.priority
            .unwrap_or(i32::MAX)
            .cmp(&right.priority.unwrap_or(i32::MAX))
            .then_with(|| left.slug.cmp(&right.slug))
    });

    Ok(response
        .models
        .into_iter()
        .filter(|model| !matches!(model.visibility.as_deref(), Some("hide" | "none")))
        .map(|model| ProviderModelInfo {
            id: model.slug,
            display_name: model.display_name.filter(|value| !value.is_empty()),
            context_window: model.context_window,
            effort_levels: model
                .supported_reasoning_levels
                .unwrap_or_default()
                .into_iter()
                .map(|level| level.effort)
                .collect(),
            default_effort: model
                .default_reasoning_level
                .filter(|value| !value.is_empty()),
        })
        .collect())
}

fn parse_account_limits_response(body: &str) -> Result<Option<AccountLimitsInfo>, ProviderError> {
    let response: ChatGptUsageResponse =
        serde_json::from_str(body).map_err(|e| ProviderError::Parse(e.to_string()))?;

    let ChatGptUsageResponse {
        plan_type,
        rate_limit,
        additional_rate_limits,
        credits,
    } = response;

    let mut limits = Vec::new();
    if rate_limit.is_some() || credits.is_some() || plan_type.is_some() {
        limits.push(AccountLimitInfo {
            limit_id: Some("codex".to_string()),
            limit_name: None,
            primary: rate_limit
                .as_ref()
                .and_then(|details| map_account_limit_window(details.primary_window.as_ref())),
            secondary: rate_limit
                .as_ref()
                .and_then(|details| map_account_limit_window(details.secondary_window.as_ref())),
            credits: credits.map(map_account_credits),
        });
    }

    limits.extend(
        additional_rate_limits
            .unwrap_or_default()
            .into_iter()
            .map(|details| AccountLimitInfo {
                limit_id: Some(details.metered_feature),
                limit_name: Some(details.limit_name),
                primary: details.rate_limit.as_ref().and_then(|rate_limit| {
                    map_account_limit_window(rate_limit.primary_window.as_ref())
                }),
                secondary: details.rate_limit.as_ref().and_then(|rate_limit| {
                    map_account_limit_window(rate_limit.secondary_window.as_ref())
                }),
                credits: None,
            }),
    );

    if plan_type.is_none() && limits.is_empty() {
        return Ok(None);
    }

    Ok(Some(AccountLimitsInfo { plan_type, limits }))
}

fn map_account_limit_window(window: Option<&ChatGptRateLimitWindow>) -> Option<AccountLimitWindow> {
    let window = window?;
    Some(AccountLimitWindow {
        used_percent: window.used_percent,
        window_minutes: window_minutes(window.limit_window_seconds),
        resets_at: window.reset_at,
    })
}

fn map_account_credits(credits: ChatGptCreditDetails) -> AccountCreditsInfo {
    AccountCreditsInfo {
        has_credits: credits.has_credits,
        unlimited: credits.unlimited,
        balance: credits.balance,
    }
}

fn window_minutes(seconds: u64) -> Option<u64> {
    (seconds > 0).then_some(seconds.div_ceil(60))
}

/// Strip configured patterns from text content
fn strip_patterns_from_text(text: &str, compat: &ProviderCompat) -> String {
    match &compat.strip_patterns {
        Some(patterns) if !patterns.is_empty() => {
            let mut result = text.to_string();
            for pattern in patterns {
                result = result.replace(pattern, "");
            }
            result
        }
        _ => text.to_string(),
    }
}

/// Strip provider-generated assistant sentinels without touching inline prose.
fn strip_assistant_text_sentinels(text: &str, compat: &ProviderCompat) -> String {
    let mut result = text.to_string();
    for pattern in compat.assistant_text_strip_patterns() {
        result = strip_line_start_sentinel_suffix(&result, pattern);
    }
    result
}

fn strip_line_start_sentinel_suffix(text: &str, pattern: &str) -> String {
    if pattern.is_empty() {
        return text.to_string();
    }

    let mut search_start = 0;
    while search_start <= text.len() {
        let Some(relative_index) = text[search_start..].find(pattern) else {
            return text.to_string();
        };
        let index = search_start + relative_index;
        let before = &text[..index];
        let line_prefix = before
            .rsplit_once('\n')
            .map(|(_, tail)| tail)
            .unwrap_or(before);

        if line_prefix.trim().is_empty() {
            return before.trim_end_matches(char::is_whitespace).to_string();
        }

        search_start = index + pattern.len();
    }

    text.to_string()
}

/// Deduplicate tool results: keep last occurrence of each tool_call_id
fn dedup_tool_results(messages: &mut Vec<Value>) {
    use std::collections::HashMap;

    // Find the last index of each tool_call_id
    let mut last_index: HashMap<String, usize> = HashMap::new();
    for (i, msg) in messages.iter().enumerate() {
        if msg["role"].as_str() == Some("tool")
            && let Some(id) = msg["tool_call_id"].as_str()
        {
            last_index.insert(id.to_string(), i);
        }
    }

    // Keep only the last occurrence
    let mut seen: HashMap<String, bool> = HashMap::new();
    let mut to_remove = Vec::new();
    for (i, msg) in messages.iter().enumerate() {
        if msg["role"].as_str() == Some("tool")
            && let Some(id) = msg["tool_call_id"].as_str()
            && let Some(&last_i) = last_index.get(id)
        {
            if i != last_i && !seen.contains_key(id) {
                to_remove.push(i);
            }
            if i == last_i {
                seen.insert(id.to_string(), true);
            }
        }
    }

    // Remove in reverse order to preserve indices
    for i in to_remove.into_iter().rev() {
        messages.remove(i);
    }
}

/// Remove tool_call entries from assistant messages that have no corresponding tool result
fn clean_orphaned_tool_calls(messages: &mut [Value]) {
    use std::collections::HashSet;

    let answered_ids: HashSet<String> = messages
        .iter()
        .filter(|m| m["role"].as_str() == Some("tool"))
        .filter_map(|m| m["tool_call_id"].as_str().map(String::from))
        .collect();

    for msg in messages.iter_mut() {
        if msg["role"].as_str() == Some("assistant")
            && let Some(tcs) = msg["tool_calls"].as_array_mut()
        {
            tcs.retain(|tc| {
                tc["id"]
                    .as_str()
                    .map(|id| answered_ids.contains(id))
                    .unwrap_or(true)
            });
            if tcs.is_empty() {
                msg.as_object_mut().unwrap().remove("tool_calls");
            }
        }
    }
}

/// Merge consecutive assistant messages into one
fn merge_consecutive_assistant(messages: &mut Vec<Value>) {
    let mut i = 0;
    while i + 1 < messages.len() {
        if messages[i]["role"].as_str() == Some("assistant")
            && messages[i + 1]["role"].as_str() == Some("assistant")
        {
            let next = messages.remove(i + 1);

            // Merge text content
            let curr_text = messages[i]["content"].as_str().unwrap_or("").to_string();
            let next_text = next["content"].as_str().unwrap_or("").to_string();
            let merged_text = match (curr_text.is_empty(), next_text.is_empty()) {
                (true, true) => String::new(),
                (true, false) => next_text,
                (false, true) => curr_text,
                (false, false) => format!("{}{}", curr_text, next_text),
            };

            if !merged_text.is_empty() {
                messages[i]["content"] = json!(merged_text);
            }

            // Merge reasoning content for thinking-mode APIs that require it
            // to be replayed alongside assistant tool calls.
            let curr_reasoning = messages[i]["reasoning_content"]
                .as_str()
                .unwrap_or("")
                .to_string();
            let next_reasoning = next["reasoning_content"].as_str().unwrap_or("").to_string();
            let merged_reasoning = match (curr_reasoning.is_empty(), next_reasoning.is_empty()) {
                (true, true) => String::new(),
                (true, false) => next_reasoning,
                (false, true) => curr_reasoning,
                (false, false) => format!("{}{}", curr_reasoning, next_reasoning),
            };

            if !merged_reasoning.is_empty() {
                messages[i]["reasoning_content"] = json!(merged_reasoning);
            }

            // Merge tool_calls
            if let Some(next_tcs) = next["tool_calls"].as_array() {
                let curr_tcs = messages[i]
                    .as_object_mut()
                    .unwrap()
                    .entry("tool_calls")
                    .or_insert_with(|| json!([]));
                if let Some(arr) = curr_tcs.as_array_mut() {
                    arr.extend(next_tcs.iter().cloned());
                }
            }

            // Don't increment i - check the merged result against the next message
        } else {
            i += 1;
        }
    }
}

/// State for accumulating tool call deltas by index
struct ToolCallAccumulator {
    id: String,
    item_id: Option<String>,
    name: String,
    arguments: String,
    extra: Option<Value>,
}

struct StreamState {
    tool_calls: Vec<ToolCallAccumulator>,
    input_tokens: u64,
    output_tokens: u64,
    saw_output_text_delta: bool,
    saw_reasoning_delta: bool,
    /// Deferred Done event: populated when finish_reason arrives, emitted on
    /// [DONE] so the final usage-only chunk has a chance to update token counts.
    pending_done: Option<LlmEvent>,
}

impl StreamState {
    fn new() -> Self {
        Self {
            tool_calls: Vec::new(),
            input_tokens: 0,
            output_tokens: 0,
            saw_output_text_delta: false,
            saw_reasoning_delta: false,
            pending_done: None,
        }
    }

    /// Emit the deferred Done event with up-to-date token counts.
    ///
    /// OpenAI sends usage in a separate trailing chunk (choices:[]) *after* the
    /// chunk that carries `finish_reason`. We defer the Done event until [DONE]
    /// so that token counts are always accurate.
    fn flush_done(&mut self) -> Option<LlmEvent> {
        let pending = self.pending_done.take()?;
        Some(match pending {
            LlmEvent::Done { stop_reason, .. } => LlmEvent::Done {
                stop_reason,
                usage: TokenUsage {
                    input_tokens: self.input_tokens,
                    output_tokens: self.output_tokens,
                    cache_creation_tokens: 0,
                    cache_read_tokens: 0,
                },
            },
            other => other,
        })
    }

    fn get_or_create_tool(&mut self, index: usize) -> &mut ToolCallAccumulator {
        while self.tool_calls.len() <= index {
            self.tool_calls.push(ToolCallAccumulator {
                id: String::new(),
                item_id: None,
                name: String::new(),
                arguments: String::new(),
                extra: None,
            });
        }
        &mut self.tool_calls[index]
    }

    fn has_pending_tool_calls(&self) -> bool {
        !self.tool_calls.is_empty()
    }

    fn get_or_create_response_tool(
        &mut self,
        output_index: Option<usize>,
        call_id: Option<&str>,
        item_id: Option<&str>,
    ) -> &mut ToolCallAccumulator {
        let preferred_id = call_id
            .filter(|value| !value.is_empty())
            .or_else(|| item_id.filter(|value| !value.is_empty()));

        if let Some(index) = output_index {
            let tool = self.get_or_create_tool(index);
            if let Some(call_id) = preferred_id
                && tool.id.is_empty()
            {
                tool.id = call_id.to_string();
            }
            if let Some(item_id) = item_id
                && tool.item_id.is_none()
            {
                tool.item_id = Some(item_id.to_string());
            }
            return tool;
        }

        if let Some(call_id) = preferred_id
            && let Some(position) = self.tool_calls.iter().position(|tool| tool.id == call_id)
        {
            let tool = &mut self.tool_calls[position];
            if let Some(item_id) = item_id
                && tool.item_id.is_none()
            {
                tool.item_id = Some(item_id.to_string());
            }
            return tool;
        }

        if let Some(item_id) = item_id
            && let Some(position) = self
                .tool_calls
                .iter()
                .position(|tool| tool.item_id.as_deref() == Some(item_id))
        {
            let tool = &mut self.tool_calls[position];
            if let Some(call_id) = preferred_id
                && tool.id.is_empty()
            {
                tool.id = call_id.to_string();
            }
            return tool;
        }

        self.tool_calls.push(ToolCallAccumulator {
            id: preferred_id.unwrap_or_default().to_string(),
            item_id: item_id.map(ToOwned::to_owned),
            name: String::new(),
            arguments: String::new(),
            extra: None,
        });
        self.tool_calls
            .last_mut()
            .expect("response tool was just pushed")
    }
}

#[async_trait]
impl LlmProvider for OpenAIProvider {
    async fn metadata(&self) -> Result<ProviderMetadata, ProviderError> {
        let mut refreshed_after_unauthorized = false;

        loop {
            let transport = self.resolve_transport().await?;
            let result = async {
                let models = self.fetch_models(&transport).await?;
                let account_limits = self.fetch_account_limits(&transport).await?;
                Ok::<_, ProviderError>(ProviderMetadata {
                    models,
                    account_limits,
                })
            }
            .await;

            match result {
                Err(ProviderError::Api { status: 401, .. })
                    if !refreshed_after_unauthorized && self.oauth.is_some() =>
                {
                    self.refresh_after_unauthorized().await?;
                    refreshed_after_unauthorized = true;
                }
                other => return other,
            }
        }
    }

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

impl OpenAIProvider {
    async fn stream_inner(
        &self,
        request: &LlmRequest,
        retry_observer: Option<RetryObserver>,
    ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        let mut transport = self.resolve_transport().await?;
        let body = if transport.use_responses_api {
            self.build_responses_request_body(
                request,
                transport.system_as_instructions,
                transport.include_max_output_tokens,
            )
        } else {
            self.build_request_body(request)
        };

        dump_request_body(&self.debug, &body);
        reset_response_dump(&self.debug);

        let mut refreshed_after_unauthorized = false;
        let response = loop {
            let url = format!("{}{}", transport.base_url, transport.api_path);
            let result = with_retry_if_notify_budget(
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
                || async {
                    post_stream_request(
                        &self.client,
                        &url,
                        &transport.headers,
                        &body,
                        request.session_id.as_deref(),
                        transport.use_responses_api && self.compat.codex_session_identity(),
                    )
                    .await
                },
            )
            .await;

            match result {
                Err(ProviderError::Api { status: 401, .. })
                    if !refreshed_after_unauthorized && self.oauth.is_some() =>
                {
                    self.refresh_after_unauthorized().await?;
                    transport = self.resolve_transport().await?;
                    refreshed_after_unauthorized = true;
                }
                Ok(response) => break response,
                Err(error) => return Err(error),
            }
        };

        let (tx, rx) = mpsc::channel(64);
        let debug = self.debug.clone();
        let compat = self.compat.clone();

        let use_responses_api = transport.use_responses_api;
        let response_retry_context = use_responses_api.then(|| ResponseApiStreamRetryContext {
            client: self.client.clone(),
            url: format!("{}{}", transport.base_url, transport.api_path),
            headers: transport.headers,
            body,
            session_id: request.session_id.clone(),
            codex_session_identity: self.compat.codex_session_identity(),
            retry_observer,
        });
        tokio::spawn(async move {
            let result = if use_responses_api {
                process_response_api_stream_with_retries(
                    response,
                    &tx,
                    response_retry_context.expect("responses retry context should exist"),
                )
                .await
            } else {
                process_sse_stream(response, &tx, &debug, &compat).await
            };
            if let Err(e) = result {
                let _ = tx.send(LlmEvent::Error(e.to_string())).await;
            }
        });

        Ok(rx)
    }
}

fn append_query_param(url: &str, name: &str, value: &str) -> String {
    let separator = if url.contains('?') { '&' } else { '?' };
    format!("{url}{separator}{name}={value}")
}

async fn post_stream_request(
    client: &reqwest::Client,
    url: &str,
    headers: &HeaderMap,
    body: &Value,
    session_id: Option<&str>,
    codex_session_identity: bool,
) -> Result<reqwest::Response, ProviderError> {
    let mut headers = headers.clone();
    apply_responses_request_identity(&mut headers, session_id, codex_session_identity)?;

    let response = client
        .post(url)
        .headers(headers)
        .json(body)
        .send()
        .await
        .map_err(|err| request_send_error("POST", url, err))?;

    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }

    let retry_after_ms = parse_retry_after_ms(response.headers());
    let body_text = response.text().await.unwrap_or_default();
    if status.as_u16() == 429 {
        return Err(ProviderError::RateLimited { retry_after_ms });
    }
    Err(ProviderError::Api {
        status: status.as_u16(),
        message: body_text,
    })
}

fn request_send_error(method: &str, url: &str, err: reqwest::Error) -> ProviderError {
    ProviderError::Connection(format!(
        "{method} {url} failed: {}",
        format_error_chain(&err)
    ))
}

fn parse_retry_after_ms(headers: &HeaderMap) -> u64 {
    if let Some(milliseconds) = headers
        .get("retry-after-ms")
        .and_then(|value| value.to_str().ok())
        .and_then(parse_duration_millis)
    {
        return milliseconds;
    }

    headers
        .get(RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(parse_retry_after_seconds)
        .unwrap_or(5000)
}

fn parse_duration_millis(value: &str) -> Option<u64> {
    value.trim().parse::<u64>().ok()
}

fn parse_retry_after_seconds(value: &str) -> Option<u64> {
    let value = value.trim();
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(seconds.saturating_mul(1000));
    }
    value
        .parse::<f64>()
        .ok()
        .filter(|seconds| seconds.is_finite() && *seconds >= 0.0)
        .map(|seconds| (seconds * 1000.0).ceil() as u64)
}

fn format_error_chain(err: &dyn std::error::Error) -> String {
    let mut message = err.to_string();
    let mut source = err.source();
    while let Some(next) = source {
        let next_message = next.to_string();
        if !next_message.is_empty() {
            message.push_str(": ");
            message.push_str(&next_message);
        }
        source = next.source();
    }
    message
}

pub(crate) fn build_chat_request_body(request: &LlmRequest, compat: &ProviderCompat) -> Value {
    let max_tokens_field = compat.max_tokens_field.as_deref().unwrap_or("max_tokens");

    let mut body = json!({
        "model": request.model,
        "messages": OpenAIProvider::build_messages(&request.messages, &request.system, compat),
        "stream": true,
        "stream_options": { "include_usage": true }
    });
    body[max_tokens_field] = json!(request.max_tokens);

    if !request.tools.is_empty() {
        body["tools"] = json!(OpenAIProvider::build_tools(&request.tools));
    }

    if let Some(effort) = &request.reasoning_effort {
        body["reasoning_effort"] = json!(effort);
    }

    body
}

pub(crate) async fn process_sse_stream(
    response: reqwest::Response,
    tx: &mpsc::Sender<LlmEvent>,
    debug: &DebugConfig,
    compat: &ProviderCompat,
) -> Result<(), ProviderError> {
    process_sse_byte_stream_with_idle_timeout(
        response.bytes_stream(),
        tx,
        debug,
        compat,
        STREAM_IDLE_TIMEOUT,
    )
    .await
}

async fn process_sse_byte_stream_with_idle_timeout<S, B, E>(
    mut stream: S,
    tx: &mpsc::Sender<LlmEvent>,
    debug: &DebugConfig,
    compat: &ProviderCompat,
    idle_timeout: std::time::Duration,
) -> Result<(), ProviderError>
where
    S: futures::Stream<Item = Result<B, E>> + Unpin,
    B: AsRef<[u8]>,
    E: std::fmt::Display,
{
    use futures::StreamExt;

    let mut state = StreamState::new();
    let mut buffer = String::new();

    loop {
        let chunk = match tokio::time::timeout(idle_timeout, stream.next()).await {
            Ok(Some(chunk)) => chunk.map_err(|e| ProviderError::Connection(e.to_string()))?,
            Ok(None) => {
                return Err(ProviderError::Connection(
                    "SSE stream closed before [DONE]".to_string(),
                ));
            }
            Err(_) => {
                return Err(ProviderError::Connection(
                    "idle timeout waiting for SSE".to_string(),
                ));
            }
        };
        let text = String::from_utf8_lossy(chunk.as_ref());
        buffer.push_str(&text);

        // Process complete lines
        while let Some(line_end) = buffer.find('\n') {
            let line = buffer[..line_end].trim().to_string();
            buffer = buffer[line_end + 1..].to_string();

            if line.is_empty() || line.starts_with(':') {
                continue;
            }

            if let Some(data) = line.strip_prefix("data: ") {
                dump_response_chunk(debug, data);
                if data == "[DONE]" {
                    // Flush the deferred Done event now that the final
                    // usage-only chunk (choices:[]) has updated token counts.
                    if let Some(done) = state.flush_done() {
                        let _ = tx.send(done).await;
                    }
                    return Ok(());
                }

                let events = parse_sse_chunk(data, &mut state, compat);
                for event in events {
                    if tx.send(event).await.is_err() {
                        return Ok(());
                    }
                }
            }
        }
    }
}

fn parse_sse_chunk(data: &str, state: &mut StreamState, compat: &ProviderCompat) -> Vec<LlmEvent> {
    let json: Value = match serde_json::from_str(data) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };

    parse_openai_chunk_json(&json, state, compat)
}

fn parse_openai_chunk_json(
    json: &Value,
    state: &mut StreamState,
    compat: &ProviderCompat,
) -> Vec<LlmEvent> {
    let mut events = Vec::new();

    // Extract usage if present
    if let Some(usage) = json.get("usage") {
        state.input_tokens = usage["prompt_tokens"]
            .as_u64()
            .unwrap_or(state.input_tokens);
        state.output_tokens = usage["completion_tokens"]
            .as_u64()
            .unwrap_or(state.output_tokens);
    }

    let Some(choice) = json["choices"].as_array().and_then(|c| c.first()) else {
        return events;
    };

    let delta = &choice["delta"];

    if let Some(reasoning) = extract_reasoning_delta(delta, compat) {
        state.saw_reasoning_delta = true;
        events.push(LlmEvent::ThinkingDelta(reasoning.to_string()));
    }
    if !state.saw_reasoning_delta
        && let Some(reasoning) = extract_reasoning_delta(&choice["message"], compat)
    {
        state.saw_reasoning_delta = true;
        events.push(LlmEvent::ThinkingDelta(reasoning.to_string()));
    }

    // Text content
    if let Some(content) = delta["content"].as_str()
        && !content.is_empty()
    {
        let content = strip_assistant_text_sentinels(content, compat);
        if !content.is_empty() {
            state.saw_output_text_delta = true;
            events.push(LlmEvent::TextDelta(content));
        }
    }

    // Tool calls
    if let Some(tool_calls) = delta["tool_calls"].as_array() {
        ingest_tool_calls(tool_calls, state, true);
    } else if let Some(tool_calls) = choice["message"]["tool_calls"].as_array() {
        ingest_tool_calls(tool_calls, state, false);
    }

    // Check finish_reason — defer Done until [DONE] so the trailing usage
    // chunk (choices:[]) can update token counts first.
    if let Some(finish_reason) = choice["finish_reason"].as_str() {
        match finish_reason {
            "tool_calls" => {
                // Emit accumulated tool calls immediately.
                for tc in state.tool_calls.drain(..) {
                    let input: Value = serde_json::from_str(&tc.arguments)
                        .unwrap_or(Value::Object(serde_json::Map::new()));
                    events.push(LlmEvent::ToolUse {
                        id: tc.id,
                        name: tc.name,
                        input,
                        extra: tc.extra,
                    });
                }
                state.pending_done = Some(LlmEvent::Done {
                    stop_reason: StopReason::ToolUse,
                    usage: TokenUsage::default(),
                });
            }
            "stop" => {
                state.pending_done = Some(LlmEvent::Done {
                    stop_reason: StopReason::EndTurn,
                    usage: TokenUsage::default(),
                });
            }
            "length" => {
                state.pending_done = Some(LlmEvent::Done {
                    stop_reason: StopReason::MaxTokens,
                    usage: TokenUsage::default(),
                });
            }
            _ => {}
        }
    }

    events
}

fn ingest_tool_calls(tool_calls: &[Value], state: &mut StreamState, use_index_field: bool) {
    for (fallback_index, tc) in tool_calls.iter().enumerate() {
        let index = if use_index_field {
            tc["index"].as_u64().unwrap_or(0) as usize
        } else {
            fallback_index
        };
        let acc = state.get_or_create_tool(index);

        if let Some(id) = tc["id"].as_str() {
            acc.id = id.to_string();
        }
        // Only overwrite when non-empty — some third-party APIs send `"name":""`
        // in every delta chunk which would erase the real name from the first chunk.
        if let Some(name) = tc["function"]["name"].as_str().filter(|n| !n.is_empty()) {
            acc.name = name.to_string();
        }
        if let Some(args) = tc["function"]["arguments"].as_str() {
            acc.arguments.push_str(args);
        }
        if let Some(extra) = tc.get("extra_content").filter(|value| !value.is_null()) {
            acc.extra = Some(extra.clone());
        }
    }
}

fn extract_reasoning_delta<'a>(delta: &'a Value, compat: &ProviderCompat) -> Option<&'a str> {
    compat
        .reasoning_delta_fields()
        .into_iter()
        .find_map(|field| {
            delta
                .get(field)
                .and_then(Value::as_str)
                .filter(|reasoning| !reasoning.is_empty())
        })
}

async fn process_response_api_stream(
    response: reqwest::Response,
    tx: &mpsc::Sender<LlmEvent>,
) -> Result<(), ProviderError> {
    process_response_api_byte_stream_with_idle_timeout(
        response.bytes_stream(),
        tx,
        STREAM_IDLE_TIMEOUT,
    )
    .await
}

struct ResponseApiStreamRetryContext {
    client: reqwest::Client,
    url: String,
    headers: HeaderMap,
    body: Value,
    session_id: Option<String>,
    codex_session_identity: bool,
    retry_observer: Option<RetryObserver>,
}

async fn process_response_api_stream_with_retries(
    first_response: reqwest::Response,
    tx: &mpsc::Sender<LlmEvent>,
    context: ResponseApiStreamRetryContext,
) -> Result<(), ProviderError> {
    let mut response = Some(first_response);
    let mut retries = 0;
    let mut backoff = std::time::Duration::from_secs(1);

    loop {
        let result = if let Some(response) = response.take() {
            process_response_api_stream(response, tx).await
        } else {
            match post_stream_request(
                &context.client,
                &context.url,
                &context.headers,
                &context.body,
                context.session_id.as_deref(),
                context.codex_session_identity,
            )
            .await
            {
                Ok(next_response) => {
                    response = Some(next_response);
                    continue;
                }
                Err(error) => Err(error),
            }
        };

        match result {
            Ok(()) => return Ok(()),
            Err(error) if error.is_retryable() => {
                let max_retries = response_api_stream_retry_budget(&error);
                if retries >= max_retries {
                    return Err(error);
                }

                retries += 1;
                let delay = retry_delay(&error, backoff);
                if let Some(observer) = context.retry_observer.as_ref() {
                    observer(RetryAttempt {
                        attempt: retries,
                        max_retries,
                        delay,
                        error: error.to_string(),
                    });
                }
                tokio::time::sleep(delay).await;
                backoff = (backoff * 2).min(std::time::Duration::from_secs(30));
            }
            Err(error) => return Err(error),
        }
    }
}

fn response_api_stream_retry_budget(error: &ProviderError) -> u32 {
    match error {
        ProviderError::RateLimited { .. } => STREAM_REQUEST_RATE_LIMIT_RETRIES,
        _ => STREAM_REQUEST_CONNECTION_RETRIES,
    }
}

async fn process_response_api_byte_stream_with_idle_timeout<S, B, E>(
    mut stream: S,
    tx: &mpsc::Sender<LlmEvent>,
    idle_timeout: std::time::Duration,
) -> Result<(), ProviderError>
where
    S: futures::Stream<Item = Result<B, E>> + Unpin,
    B: AsRef<[u8]>,
    E: std::fmt::Display,
{
    use futures::StreamExt;

    let mut state = StreamState::new();
    let mut buffer = String::new();
    let mut current_event: Option<String> = None;
    let mut current_data = String::new();
    let mut saw_terminal = false;
    let mut emitted_events = false;

    loop {
        let chunk = match tokio::time::timeout(idle_timeout, stream.next()).await {
            Ok(Some(chunk)) => match chunk {
                Ok(chunk) => chunk,
                Err(error) => {
                    return finish_response_api_stream_error(
                        tx,
                        emitted_events,
                        ProviderError::Connection(error.to_string()),
                    )
                    .await;
                }
            },
            Ok(None) => break,
            Err(_) => {
                return finish_response_api_stream_error(
                    tx,
                    emitted_events,
                    ProviderError::Connection("idle timeout waiting for SSE".to_string()),
                )
                .await;
            }
        };
        let text = String::from_utf8_lossy(chunk.as_ref());
        buffer.push_str(&text);

        while let Some(line_end) = buffer.find('\n') {
            let line = buffer[..line_end].trim_end_matches('\r').to_string();
            buffer = buffer[line_end + 1..].to_string();

            if line.is_empty() {
                if !current_data.is_empty() {
                    let parsed = parse_response_api_event(
                        current_event.as_deref().unwrap_or_default(),
                        &current_data,
                        &mut state,
                    );
                    if let Some(error) = parsed.error {
                        if !emitted_events {
                            return Err(error);
                        }
                        let message = parsed
                            .events
                            .into_iter()
                            .find_map(|event| match event {
                                LlmEvent::Error(message) => Some(message),
                                _ => None,
                            })
                            .unwrap_or_else(|| error.to_string());
                        let _ = tx.send(LlmEvent::Error(message)).await;
                        return Ok(());
                    }
                    for event in parsed.events {
                        emitted_events = true;
                        if tx.send(event).await.is_err() {
                            return Ok(());
                        }
                    }
                    if parsed.terminal {
                        return Ok(());
                    }
                }
                current_event = None;
                current_data.clear();
                continue;
            }

            if let Some(event) = line.strip_prefix("event: ") {
                current_event = Some(event.to_string());
                continue;
            }

            if let Some(data) = line.strip_prefix("data: ") {
                if data == "[DONE]" {
                    if let Some(done) = state.flush_done() {
                        let _ = tx.send(done).await;
                    } else if !saw_terminal {
                        return finish_response_api_stream_error(
                            tx,
                            emitted_events,
                            ProviderError::Connection(
                                "Responses stream closed before response.completed".to_string(),
                            ),
                        )
                        .await;
                    }
                    return Ok(());
                }
                if !current_data.is_empty() {
                    current_data.push('\n');
                }
                current_data.push_str(data);
            }
        }
    }

    if !current_data.is_empty() {
        let parsed = parse_response_api_event(
            current_event.as_deref().unwrap_or_default(),
            &current_data,
            &mut state,
        );
        if let Some(error) = parsed.error {
            if !emitted_events {
                return Err(error);
            }
            let message = parsed
                .events
                .into_iter()
                .find_map(|event| match event {
                    LlmEvent::Error(message) => Some(message),
                    _ => None,
                })
                .unwrap_or_else(|| error.to_string());
            let _ = tx.send(LlmEvent::Error(message)).await;
            return Ok(());
        }
        if !parsed.terminal && !parsed.events.is_empty() {
            emitted_events = true;
        }
        for event in parsed.events {
            if tx.send(event).await.is_err() {
                return Ok(());
            }
        }
        saw_terminal = parsed.terminal;
    }

    if let Some(done) = state.flush_done() {
        let _ = tx.send(done).await;
    } else if !saw_terminal {
        return finish_response_api_stream_error(
            tx,
            emitted_events,
            ProviderError::Connection(
                "Responses stream closed before response.completed".to_string(),
            ),
        )
        .await;
    }

    Ok(())
}

async fn finish_response_api_stream_error(
    tx: &mpsc::Sender<LlmEvent>,
    emitted_events: bool,
    error: ProviderError,
) -> Result<(), ProviderError> {
    if !emitted_events {
        return Err(error);
    }

    let _ = tx.send(LlmEvent::Error(error.to_string())).await;
    Ok(())
}

#[derive(Default)]
struct ParsedResponseApiEvent {
    events: Vec<LlmEvent>,
    terminal: bool,
    error: Option<ProviderError>,
}

fn parse_response_api_event(
    event: &str,
    data: &str,
    state: &mut StreamState,
) -> ParsedResponseApiEvent {
    let json: Value = match serde_json::from_str(data) {
        Ok(value) => value,
        Err(_) => return ParsedResponseApiEvent::default(),
    };
    let event_name = if event.is_empty() {
        json.get("type").and_then(Value::as_str).unwrap_or_default()
    } else {
        event
    };

    match event_name {
        "response.output_text.delta" => {
            let Some(delta) = json
                .get("delta")
                .or_else(|| json.get("text"))
                .or_else(|| json.get("output_text_delta"))
                .and_then(Value::as_str)
                .filter(|delta| !delta.is_empty())
            else {
                return ParsedResponseApiEvent::default();
            };
            state.saw_output_text_delta = true;
            ParsedResponseApiEvent {
                events: vec![LlmEvent::TextDelta(delta.to_string())],
                terminal: false,
                error: None,
            }
        }
        "response.reasoning_text.delta" | "response.reasoning_summary_text.delta" => {
            let Some(delta) = json
                .get("delta")
                .and_then(Value::as_str)
                .filter(|delta| !delta.is_empty())
            else {
                return ParsedResponseApiEvent::default();
            };
            state.saw_reasoning_delta = true;
            ParsedResponseApiEvent {
                events: vec![LlmEvent::ThinkingDelta(delta.to_string())],
                terminal: false,
                error: None,
            }
        }
        "response.output_item.added" => {
            handle_response_output_item_added(&json, state);
            ParsedResponseApiEvent::default()
        }
        "response.function_call_arguments.delta" => {
            handle_response_function_call_arguments_delta(&json, state);
            ParsedResponseApiEvent::default()
        }
        "response.output_item.done" => ParsedResponseApiEvent {
            events: handle_response_output_item_done(&json, state),
            terminal: false,
            error: None,
        },
        "response.completed" => parse_response_api_terminal_event(&json, state, true),
        "response.incomplete" => parse_response_api_terminal_event(&json, state, false),
        "response.failed" => parse_response_api_failed_event(&json),
        _ => ParsedResponseApiEvent::default(),
    }
}

fn parse_response_api_failed_event(json: &Value) -> ParsedResponseApiEvent {
    let message = extract_response_api_error_message(json);
    ParsedResponseApiEvent {
        events: vec![LlmEvent::Error(message)],
        terminal: true,
        error: response_api_failed_provider_error(json),
    }
}

fn handle_response_output_item_added(json: &Value, state: &mut StreamState) {
    let Some(item) = json.get("item") else {
        return;
    };
    if item.get("type").and_then(Value::as_str) != Some("function_call") {
        return;
    }

    let output_index = json
        .get("output_index")
        .and_then(Value::as_u64)
        .map(|index| index as usize);
    let call_id = item
        .get("call_id")
        .or_else(|| item.get("id"))
        .and_then(Value::as_str);
    let item_id = item.get("id").and_then(Value::as_str);
    let tool = state.get_or_create_response_tool(output_index, call_id, item_id);

    if let Some(name) = item.get("name").and_then(Value::as_str) {
        tool.name = name.to_string();
    }
}

fn handle_response_function_call_arguments_delta(json: &Value, state: &mut StreamState) {
    let Some(delta) = json
        .get("delta")
        .or_else(|| json.get("arguments_delta"))
        .and_then(Value::as_str)
    else {
        return;
    };

    let output_index = json
        .get("output_index")
        .and_then(Value::as_u64)
        .map(|index| index as usize);
    let call_id = json.get("call_id").and_then(Value::as_str);
    let item_id = json.get("item_id").and_then(Value::as_str);
    let tool = state.get_or_create_response_tool(output_index, call_id, item_id);
    tool.arguments.push_str(delta);
}

fn handle_response_output_item_done(json: &Value, state: &mut StreamState) -> Vec<LlmEvent> {
    let Some(item) = json.get("item") else {
        return Vec::new();
    };

    match item.get("type").and_then(Value::as_str) {
        Some("function_call") => {
            let output_index = json
                .get("output_index")
                .and_then(Value::as_u64)
                .map(|index| index as usize);
            let call_id = item
                .get("call_id")
                .or_else(|| item.get("id"))
                .and_then(Value::as_str);
            let item_id = item.get("id").and_then(Value::as_str);
            let tool = state.get_or_create_response_tool(output_index, call_id, item_id);
            if let Some(name) = item.get("name").and_then(Value::as_str) {
                tool.name = name.to_string();
            }
            if let Some(arguments) = item.get("arguments").and_then(Value::as_str) {
                tool.arguments = arguments.to_string();
            }
            Vec::new()
        }
        Some("message") => {
            if state.saw_output_text_delta {
                return Vec::new();
            }
            let text = item
                .get("content")
                .and_then(Value::as_array)
                .map(|content| {
                    content
                        .iter()
                        .filter(|part| {
                            part.get("type").and_then(Value::as_str) == Some("output_text")
                        })
                        .filter_map(|part| part.get("text").and_then(Value::as_str))
                        .collect::<Vec<_>>()
                        .join("")
                })
                .unwrap_or_default();
            if text.is_empty() {
                Vec::new()
            } else {
                vec![LlmEvent::TextDelta(text)]
            }
        }
        Some("reasoning") => {
            if state.saw_reasoning_delta {
                return Vec::new();
            }
            let content_text = item
                .get("content")
                .and_then(Value::as_array)
                .map(|content| {
                    content
                        .iter()
                        .filter_map(|part| part.get("text").and_then(Value::as_str))
                        .collect::<Vec<_>>()
                        .join("")
                })
                .unwrap_or_default();
            let text = if content_text.is_empty() {
                item.get("summary")
                    .and_then(Value::as_array)
                    .map(|summary| {
                        summary
                            .iter()
                            .filter_map(|part| part.get("text").and_then(Value::as_str))
                            .collect::<Vec<_>>()
                            .join("")
                    })
                    .unwrap_or_default()
            } else {
                content_text
            };
            if text.is_empty() {
                Vec::new()
            } else {
                vec![LlmEvent::ThinkingDelta(text)]
            }
        }
        _ => Vec::new(),
    }
}

fn parse_response_api_terminal_event(
    json: &Value,
    state: &mut StreamState,
    completed: bool,
) -> ParsedResponseApiEvent {
    let Some(stop_reason) = response_api_stop_reason(json, state, completed) else {
        let reason = json
            .get("response")
            .and_then(|response| response.get("incomplete_details"))
            .and_then(|details| details.get("reason"))
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        return ParsedResponseApiEvent {
            events: vec![LlmEvent::Error(format!(
                "Incomplete response returned, reason: {reason}"
            ))],
            terminal: true,
            error: None,
        };
    };

    let mut events = Vec::new();
    if matches!(stop_reason, StopReason::ToolUse) {
        for tool_call in state.tool_calls.drain(..) {
            let input: Value = serde_json::from_str(&tool_call.arguments)
                .unwrap_or_else(|_| Value::Object(serde_json::Map::new()));
            events.push(LlmEvent::ToolUse {
                id: tool_call.id,
                name: tool_call.name,
                input,
                extra: tool_call.extra,
            });
        }
    } else {
        state.tool_calls.clear();
    }

    events.push(LlmEvent::Done {
        stop_reason,
        usage: response_api_usage(json, state),
    });
    ParsedResponseApiEvent {
        events,
        terminal: true,
        error: None,
    }
}

fn response_api_stop_reason(
    json: &Value,
    state: &StreamState,
    completed: bool,
) -> Option<StopReason> {
    let response = json.get("response").unwrap_or(json);
    let explicit = response
        .get("stop_reason")
        .or_else(|| json.get("stop_reason"))
        .and_then(Value::as_str);
    let incomplete_reason = response
        .get("incomplete_details")
        .and_then(|details| details.get("reason"))
        .or_else(|| {
            json.get("incomplete_details")
                .and_then(|details| details.get("reason"))
        })
        .and_then(Value::as_str);

    match explicit.or(incomplete_reason) {
        Some("tool_call") | Some("tool_calls") => Some(StopReason::ToolUse),
        Some("length") | Some("max_output_tokens") => Some(StopReason::MaxTokens),
        Some("stop") => Some(StopReason::EndTurn),
        Some(_) if completed && state.has_pending_tool_calls() => Some(StopReason::ToolUse),
        Some(_) if completed => Some(StopReason::EndTurn),
        Some(_) => None,
        None if completed && state.has_pending_tool_calls() => Some(StopReason::ToolUse),
        None if completed => Some(StopReason::EndTurn),
        None => None,
    }
}

fn response_api_usage(json: &Value, state: &mut StreamState) -> TokenUsage {
    let usage = json
        .get("response")
        .and_then(|response| response.get("usage"))
        .or_else(|| json.get("usage"));

    state.input_tokens = usage
        .and_then(|usage| usage.get("input_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(state.input_tokens);
    state.output_tokens = usage
        .and_then(|usage| usage.get("output_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(state.output_tokens);

    TokenUsage {
        input_tokens: state.input_tokens,
        output_tokens: state.output_tokens,
        cache_creation_tokens: 0,
        cache_read_tokens: 0,
    }
}

fn extract_response_api_error_message(json: &Value) -> String {
    let code = response_api_error_code(json);
    let message = response_api_error(json)
        .and_then(|error| error.get("message"))
        .and_then(Value::as_str)
        .unwrap_or("Responses API request failed.");

    match code {
        Some(code) if !code.is_empty() => format!("{code}: {message}"),
        _ => message.to_string(),
    }
}

fn response_api_failed_provider_error(json: &Value) -> Option<ProviderError> {
    let code = response_api_error_code(json)?;
    let message = extract_response_api_error_message(json);
    match code {
        "rate_limit_exceeded" => Some(ProviderError::RateLimited {
            retry_after_ms: 5000,
        }),
        "server_error" => Some(ProviderError::Api {
            status: 500,
            message,
        }),
        "server_is_overloaded" | "slow_down" => Some(ProviderError::Api {
            status: 503,
            message,
        }),
        _ => None,
    }
}

fn response_api_error(json: &Value) -> Option<&Value> {
    json.get("response")
        .and_then(|response| response.get("error"))
        .or_else(|| json.get("error"))
}

fn response_api_error_code(json: &Value) -> Option<&str> {
    response_api_error(json)
        .and_then(|error| error.get("code"))
        .and_then(Value::as_str)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use aion_config::auth::{
        AuthConfig, AuthStore, ChatgptAuth, ChatgptTokens, OAuthManager, StoredAuth,
    };
    use aion_config::debug::DebugConfig;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn no_compat() -> ProviderCompat {
        ProviderCompat::default()
    }

    fn openai_compat() -> ProviderCompat {
        ProviderCompat::openai_defaults()
    }

    fn chatgpt_compat() -> ProviderCompat {
        ProviderCompat {
            codex_session_identity: Some(true),
            ..ProviderCompat::openai_defaults()
        }
    }

    fn unique_test_dir(name: &str) -> PathBuf {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "aionrs-openai-{name}-{}-{timestamp}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_chatgpt_auth_files(dir: &Path, provider_id: &str) {
        let mut store = AuthStore::default();
        store.providers.insert(
            provider_id.to_string(),
            StoredAuth::chatgpt(ChatgptAuth {
                openai_api_key: None,
                auth_mode: "chatgpt".to_string(),
                last_refresh: String::new(),
                tokens: ChatgptTokens {
                    access_token: "stale-access".to_string(),
                    account_id: "acct-old".to_string(),
                    id_token: "stale-id".to_string(),
                },
            }),
        );
        std::fs::write(
            dir.join("auth.json"),
            serde_json::to_string_pretty(&store).unwrap(),
        )
        .unwrap();

        let private_store = json!({
            "providers": {
                provider_id: {
                    "refresh_token": "refresh-1",
                    "expires_at": (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339(),
                    "token_type": "Bearer"
                }
            }
        });
        std::fs::write(
            dir.join("auth-private.json"),
            serde_json::to_string_pretty(&private_store).unwrap(),
        )
        .unwrap();
    }

    async fn collect_provider_events(mut rx: mpsc::Receiver<LlmEvent>) -> Vec<LlmEvent> {
        let mut events = Vec::new();
        while let Some(event) = rx.recv().await {
            events.push(event);
        }
        events
    }

    fn chatgpt_test_provider(
        server: &MockServer,
        auth_config: AuthConfig,
        auth_dir: &Path,
    ) -> OpenAIProvider {
        let oauth = OAuthManager::new_with_credentials_paths(
            "chatgpt",
            auth_config.clone(),
            auth_dir.join("auth.json"),
            auth_dir.join("auth-private.json"),
        );
        OpenAIProvider {
            client: reqwest::Client::new(),
            api_key: String::new(),
            base_url: server.uri(),
            auth: Some(auth_config),
            oauth: Some(oauth),
            compat: chatgpt_compat(),
            debug: DebugConfig::default(),
        }
    }

    fn chatgpt_stream_request() -> LlmRequest {
        LlmRequest {
            session_id: None,
            model: "gpt-5-codex".to_string(),
            system: String::new(),
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

    // --- max_tokens_field ---

    #[test]
    fn test_max_tokens_field_default() {
        let provider = OpenAIProvider::new(
            "openai",
            "key",
            "http://localhost",
            openai_compat(),
            DebugConfig::default(),
            None,
        );
        let req = LlmRequest {
            session_id: None,
            model: "gpt-4o".into(),
            system: String::new(),
            messages: vec![],
            tools: vec![],
            max_tokens: 1024,
            thinking: None,
            reasoning_effort: None,
        };
        let body = provider.build_request_body(&req);
        assert_eq!(body["max_tokens"], 1024);
        assert!(body.get("max_completion_tokens").is_none());
    }

    #[test]
    fn test_max_tokens_field_custom() {
        let compat = ProviderCompat {
            max_tokens_field: Some("max_completion_tokens".into()),
            ..Default::default()
        };
        let provider = OpenAIProvider::new(
            "openai",
            "key",
            "http://localhost",
            compat,
            DebugConfig::default(),
            None,
        );
        let req = LlmRequest {
            session_id: None,
            model: "gpt-4o".into(),
            system: String::new(),
            messages: vec![],
            tools: vec![],
            max_tokens: 2048,
            thinking: None,
            reasoning_effort: None,
        };
        let body = provider.build_request_body(&req);
        assert_eq!(body["max_completion_tokens"], 2048);
        assert!(body.get("max_tokens").is_none());
    }

    #[test]
    fn test_build_responses_request_body_uses_instructions() {
        let provider = OpenAIProvider::new(
            "chatgpt",
            "",
            "https://api.openai.com",
            chatgpt_compat(),
            DebugConfig::default(),
            Some(aion_config::auth::AuthConfig::for_provider("chatgpt").unwrap()),
        );
        let req = LlmRequest {
            session_id: None,
            model: "gpt-5-codex".into(),
            system: "System prompt".into(),
            messages: vec![Message::new(
                Role::User,
                vec![ContentBlock::Text {
                    text: "Hello".into(),
                }],
            )],
            tools: vec![],
            max_tokens: 256,
            thinking: None,
            reasoning_effort: Some("medium".into()),
        };

        let body = provider.build_responses_request_body(&req, true, true);
        assert_eq!(body["instructions"], "System prompt");
        assert_eq!(body["max_output_tokens"], 256);
        assert_eq!(body["store"], false);
        assert_eq!(body["input"][0]["role"], "user");
        assert_eq!(body["input"][0]["content"][0]["type"], "input_text");
        assert_eq!(body["reasoning"]["effort"], "medium");
    }

    #[test]
    fn test_build_responses_request_body_includes_codex_session_identity() {
        let provider = OpenAIProvider::new(
            "chatgpt",
            "",
            "https://api.openai.com",
            chatgpt_compat(),
            DebugConfig::default(),
            Some(aion_config::auth::AuthConfig::for_provider("chatgpt").unwrap()),
        );
        let req = LlmRequest {
            session_id: Some("sess-abc".into()),
            model: "gpt-5-codex".into(),
            system: String::new(),
            messages: vec![Message::new(
                Role::User,
                vec![ContentBlock::Text {
                    text: "Hello".into(),
                }],
            )],
            tools: vec![],
            max_tokens: 256,
            thinking: None,
            reasoning_effort: None,
        };

        let body = provider.build_responses_request_body(&req, true, false);

        assert_eq!(body["prompt_cache_key"], "sess-abc");
        assert!(
            body["client_metadata"]["x-codex-installation-id"]
                .as_str()
                .is_some_and(|value| !value.is_empty())
        );
    }

    #[test]
    fn test_build_responses_request_body_skips_codex_identity_when_disabled() {
        let provider = OpenAIProvider::new(
            "openai",
            "key",
            "https://api.openai.com",
            openai_compat(),
            DebugConfig::default(),
            None,
        );
        let req = LlmRequest {
            session_id: Some("sess-abc".into()),
            model: "gpt-4.1".into(),
            system: String::new(),
            messages: vec![Message::new(
                Role::User,
                vec![ContentBlock::Text {
                    text: "Hello".into(),
                }],
            )],
            tools: vec![],
            max_tokens: 256,
            thinking: None,
            reasoning_effort: None,
        };

        let body = provider.build_responses_request_body(&req, true, false);

        assert!(body.get("prompt_cache_key").is_none());
        assert!(body.get("client_metadata").is_none());
    }

    #[test]
    fn test_apply_responses_request_identity_adds_codex_headers() {
        let mut headers = HeaderMap::new();

        apply_responses_request_identity(&mut headers, Some("sess-abc"), true).unwrap();

        assert_eq!(headers["session_id"], "sess-abc");
        assert_eq!(headers["x-client-request-id"], "sess-abc");
        assert_eq!(headers["x-codex-window-id"], "sess-abc:0");
        assert_eq!(headers["version"], CHATGPT_CLIENT_VERSION);
    }

    #[test]
    fn test_apply_responses_request_identity_skips_when_disabled() {
        let mut headers = HeaderMap::new();

        apply_responses_request_identity(&mut headers, Some("sess-abc"), false).unwrap();

        assert!(headers.is_empty());
    }

    #[test]
    fn test_build_responses_request_body_skips_empty_call_ids() {
        let provider = OpenAIProvider::new(
            "chatgpt",
            "",
            "https://api.openai.com",
            openai_compat(),
            DebugConfig::default(),
            Some(aion_config::auth::AuthConfig::for_provider("chatgpt").unwrap()),
        );
        let req = LlmRequest {
            session_id: None,
            model: "gpt-5-codex".into(),
            system: String::new(),
            messages: vec![
                Message::new(
                    Role::Assistant,
                    vec![ContentBlock::ToolUse {
                        id: String::new(),
                        name: "read_file".into(),
                        input: json!({"path": "README.md"}),
                        extra: None,
                    }],
                ),
                Message::new(
                    Role::Tool,
                    vec![ContentBlock::ToolResult {
                        tool_use_id: String::new(),
                        content: "file contents".into(),
                        is_error: false,
                    }],
                ),
            ],
            tools: vec![],
            max_tokens: 256,
            thinking: None,
            reasoning_effort: None,
        };

        let body = provider.build_responses_request_body(&req, true, true);
        let input = body["input"]
            .as_array()
            .expect("responses input should be an array");
        assert!(
            input.is_empty(),
            "empty call ids should be omitted from input"
        );
    }

    #[test]
    fn test_build_headers_merge_auth_headers_and_account_id() {
        let provider = OpenAIProvider::new(
            "chatgpt",
            "",
            "https://chatgpt.com",
            openai_compat(),
            DebugConfig::default(),
            Some(AuthConfig::for_provider("chatgpt").unwrap()),
        );

        let headers = provider
            .build_headers(
                "test-token",
                provider.auth.as_ref(),
                Some("ChatGPT-Account-Id"),
                Some("acct_123"),
            )
            .unwrap();

        assert_eq!(headers[AUTHORIZATION], "Bearer test-token");
        assert_eq!(headers["Accept"], "text/event-stream");
        assert_eq!(headers["originator"], "aionrs");
        assert_eq!(headers["ChatGPT-Account-Id"], "acct_123");
        assert!(!headers.contains_key(CONNECTION));
        assert!(headers.contains_key(USER_AGENT));
    }

    #[tokio::test]
    async fn test_stream_refreshes_chatgpt_token_once_after_401() {
        let server = MockServer::start().await;
        let auth_dir = unique_test_dir("unauthorized-refresh");
        write_chatgpt_auth_files(&auth_dir, "chatgpt");

        let mut auth_config = AuthConfig::for_provider("chatgpt").unwrap();
        auth_config.token_url = format!("{}/oauth/token", server.uri());
        auth_config.api_base_url = Some(server.uri());
        auth_config.api_path = Some("/backend-api/codex/responses".to_string());

        Mock::given(method("POST"))
            .and(path("/oauth/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "access_token": "fresh-access",
                "refresh_token": "refresh-2",
                "expires_in": 3600,
                "token_type": "Bearer"
            })))
            .mount(&server)
            .await;

        Mock::given(method("POST"))
            .and(path("/backend-api/codex/responses"))
            .and(header("authorization", "Bearer stale-access"))
            .and(header("session_id", "sess-abc"))
            .and(header("x-client-request-id", "sess-abc"))
            .and(header("x-codex-window-id", "sess-abc:0"))
            .respond_with(ResponseTemplate::new(401).set_body_string("expired token"))
            .up_to_n_times(1)
            .with_priority(1)
            .mount(&server)
            .await;

        let sse_body = concat!(
            "event: response.output_text.delta\n",
            "data: {\"delta\":\"Recovered\"}\n\n",
            "event: response.completed\n",
            "data: {\"response\":{\"usage\":{\"input_tokens\":1,\"output_tokens\":1}}}\n\n",
        );
        Mock::given(method("POST"))
            .and(path("/backend-api/codex/responses"))
            .and(header("authorization", "Bearer fresh-access"))
            .and(header("session_id", "sess-abc"))
            .and(header("x-client-request-id", "sess-abc"))
            .and(header("x-codex-window-id", "sess-abc:0"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(sse_body, "text/event-stream"))
            .with_priority(2)
            .mount(&server)
            .await;

        let oauth = OAuthManager::new_with_credentials_paths(
            "chatgpt",
            auth_config.clone(),
            auth_dir.join("auth.json"),
            auth_dir.join("auth-private.json"),
        );
        let provider = OpenAIProvider {
            client: reqwest::Client::new(),
            api_key: String::new(),
            base_url: server.uri(),
            auth: Some(auth_config),
            oauth: Some(oauth),
            compat: chatgpt_compat(),
            debug: DebugConfig::default(),
        };

        let rx = provider
            .stream(&LlmRequest {
                session_id: Some("sess-abc".to_string()),
                model: "gpt-5-codex".to_string(),
                system: String::new(),
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
            })
            .await
            .unwrap();
        let events = collect_provider_events(rx).await;

        assert!(matches!(&events[0], LlmEvent::TextDelta(text) if text == "Recovered"));
        assert!(matches!(&events[1], LlmEvent::Done { .. }));

        let requests = server.received_requests().await.unwrap();
        let response_requests = requests
            .iter()
            .filter(|request| request.url.path() == "/backend-api/codex/responses")
            .count();
        assert_eq!(response_requests, 2);
    }

    #[tokio::test]
    async fn test_resolve_transport_api_key_ignores_chatgpt_auth_overrides() {
        let provider = OpenAIProvider::new(
            "openai",
            "test-key",
            "https://api.openai.com",
            openai_compat(),
            DebugConfig::default(),
            Some(AuthConfig::for_provider("openai").unwrap()),
        );

        let transport = provider.resolve_transport().await.unwrap();

        assert_eq!(transport.base_url, "https://api.openai.com");
        assert_eq!(transport.api_path, "/v1/chat/completions");
        assert!(!transport.use_responses_api);
        assert!(!transport.system_as_instructions);
        assert!(transport.model_discovery_path.is_none());
        assert!(transport.usage_path.is_none());
        assert!(!transport.headers.contains_key("originator"));
    }

    #[test]
    fn test_parse_provider_models_response_maps_chatgpt_models() {
        let models = parse_provider_models_response(
            r#"{
                "models": [
                    {
                        "slug": "gpt-5-codex",
                        "display_name": "GPT-5 Codex",
                        "default_reasoning_level": "medium",
                        "supported_reasoning_levels": [
                            { "effort": "low" },
                            { "effort": "medium" },
                            { "effort": "high" }
                        ],
                        "context_window": 272000,
                        "visibility": "list",
                        "priority": 1
                    },
                    {
                        "slug": "hidden-model",
                        "display_name": "Hidden",
                        "supported_reasoning_levels": null,
                        "visibility": "hide",
                        "priority": 99
                    }
                ]
            }"#,
        )
        .unwrap();

        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "gpt-5-codex");
        assert_eq!(models[0].display_name.as_deref(), Some("GPT-5 Codex"));
        assert_eq!(models[0].context_window, Some(272_000));
        assert_eq!(
            models[0].effort_levels,
            vec!["low".to_string(), "medium".to_string(), "high".to_string()]
        );
        assert_eq!(models[0].default_effort.as_deref(), Some("medium"));
    }

    #[test]
    fn test_parse_account_limits_response_maps_usage_payload() {
        let account_limits = parse_account_limits_response(
            r#"{
                "plan_type": "pro",
                "rate_limit": {
                    "primary_window": {
                        "used_percent": 42,
                        "limit_window_seconds": 300,
                        "reset_at": 123
                    },
                    "secondary_window": {
                        "used_percent": 84,
                        "limit_window_seconds": 3600,
                        "reset_at": 456
                    }
                },
                "additional_rate_limits": [
                    {
                        "limit_name": "codex_other",
                        "metered_feature": "codex_other",
                        "rate_limit": {
                            "primary_window": {
                                "used_percent": 70,
                                "limit_window_seconds": 900,
                                "reset_at": 789
                            }
                        }
                    }
                ],
                "credits": {
                    "has_credits": true,
                    "unlimited": false,
                    "balance": "9.99"
                }
            }"#,
        )
        .unwrap()
        .unwrap();

        assert_eq!(account_limits.plan_type.as_deref(), Some("pro"));
        assert_eq!(account_limits.limits.len(), 2);
        assert_eq!(account_limits.limits[0].limit_id.as_deref(), Some("codex"));
        assert_eq!(
            account_limits.limits[0]
                .primary
                .as_ref()
                .map(|window| window.used_percent),
            Some(42.0)
        );
        assert_eq!(
            account_limits.limits[0]
                .primary
                .as_ref()
                .and_then(|window| window.window_minutes),
            Some(5)
        );
        assert_eq!(
            account_limits.limits[0]
                .credits
                .as_ref()
                .and_then(|credits| credits.balance.as_deref()),
            Some("9.99")
        );
        assert_eq!(
            account_limits.limits[1].limit_id.as_deref(),
            Some("codex_other")
        );
        assert_eq!(
            account_limits.limits[1]
                .primary
                .as_ref()
                .and_then(|window| window.window_minutes),
            Some(15)
        );
    }

    #[test]
    fn test_append_query_param_adds_and_extends_queries() {
        assert_eq!(
            append_query_param(
                "https://chatgpt.com/backend-api/codex/models",
                "client_version",
                "0.1.9"
            ),
            "https://chatgpt.com/backend-api/codex/models?client_version=0.1.9"
        );
        assert_eq!(
            append_query_param(
                "https://chatgpt.com/backend-api/codex/models?etag=abc",
                "client_version",
                "0.1.9"
            ),
            "https://chatgpt.com/backend-api/codex/models?etag=abc&client_version=0.1.9"
        );
    }

    // --- merge_assistant_messages ---

    #[test]
    fn test_merge_assistant_messages_enabled() {
        let messages = vec![
            Message::new(
                Role::Assistant,
                vec![ContentBlock::Text {
                    text: "hello".into(),
                }],
            ),
            Message::new(
                Role::Assistant,
                vec![ContentBlock::Text {
                    text: " world".into(),
                }],
            ),
        ];
        let result = OpenAIProvider::build_messages(&messages, "", &openai_compat());
        let assistant_msgs: Vec<_> = result.iter().filter(|m| m["role"] == "assistant").collect();
        assert_eq!(assistant_msgs.len(), 1);
        assert_eq!(assistant_msgs[0]["content"], "hello world");
    }

    #[test]
    fn test_merge_assistant_messages_disabled() {
        let messages = vec![
            Message::new(
                Role::Assistant,
                vec![ContentBlock::Text {
                    text: "hello".into(),
                }],
            ),
            Message::new(
                Role::Assistant,
                vec![ContentBlock::Text {
                    text: " world".into(),
                }],
            ),
        ];
        let result = OpenAIProvider::build_messages(&messages, "", &no_compat());
        let assistant_msgs: Vec<_> = result.iter().filter(|m| m["role"] == "assistant").collect();
        assert_eq!(assistant_msgs.len(), 2);
    }

    #[test]
    fn test_response_event_text_and_completed_map_to_events() {
        let mut state = StreamState::new();
        let text_events = parse_response_api_event(
            "response.output_text.delta",
            r#"{"delta":"Hello","response":{"id":"resp_1","model":"gpt-5-codex"}}"#,
            &mut state,
        );
        assert_eq!(text_events.events.len(), 1);
        assert!(!text_events.terminal);
        match &text_events.events[0] {
            LlmEvent::TextDelta(text) => assert_eq!(text, "Hello"),
            other => panic!("expected TextDelta, got {:?}", other),
        }

        let done_events = parse_response_api_event(
            "response.completed",
            r#"{"response":{"id":"resp_1","model":"gpt-5-codex","stop_reason":"stop","usage":{"input_tokens":12,"output_tokens":5}}}"#,
            &mut state,
        );
        assert!(done_events.terminal);
        assert_eq!(done_events.events.len(), 1);
        match &done_events.events[0] {
            LlmEvent::Done { stop_reason, usage } => {
                assert_eq!(*stop_reason, StopReason::EndTurn);
                assert_eq!(usage.input_tokens, 12);
                assert_eq!(usage.output_tokens, 5);
            }
            other => panic!("expected Done event, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_response_api_stream_idle_timeout_returns_connection_error() {
        tokio::time::pause();
        let (tx, _rx) = mpsc::channel(1);

        let result = process_response_api_byte_stream_with_idle_timeout(
            futures::stream::pending::<Result<Vec<u8>, std::io::Error>>(),
            &tx,
            Duration::from_secs(1),
        )
        .await;

        let error = result.unwrap_err();
        assert!(error.to_string().contains("idle timeout waiting for SSE"));
    }

    #[tokio::test]
    async fn test_response_api_stream_retries_server_error_before_output() {
        let server = MockServer::start().await;
        let auth_dir = unique_test_dir("response-failed-retry");
        write_chatgpt_auth_files(&auth_dir, "chatgpt");

        let mut auth_config = AuthConfig::for_provider("chatgpt").unwrap();
        auth_config.api_base_url = Some(server.uri());
        auth_config.api_path = Some("/backend-api/codex/responses".to_string());

        let failed_body = concat!(
            "event: response.failed\n",
            "data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"code\":\"server_error\",\"message\":\"An error occurred while processing your request.\"}}}\n\n",
        );

        Mock::given(method("POST"))
            .and(path("/backend-api/codex/responses"))
            .and(header("authorization", "Bearer stale-access"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(failed_body, "text/event-stream"))
            .up_to_n_times(1)
            .with_priority(1)
            .mount(&server)
            .await;

        let recovered_body = concat!(
            "event: response.output_text.delta\n",
            "data: {\"delta\":\"Recovered\"}\n\n",
            "event: response.completed\n",
            "data: {\"response\":{\"id\":\"resp_2\",\"stop_reason\":\"stop\",\"usage\":{\"input_tokens\":4,\"output_tokens\":2}}}\n\n",
        );

        Mock::given(method("POST"))
            .and(path("/backend-api/codex/responses"))
            .and(header("authorization", "Bearer stale-access"))
            .respond_with(
                ResponseTemplate::new(200).set_body_raw(recovered_body, "text/event-stream"),
            )
            .with_priority(2)
            .mount(&server)
            .await;

        let provider = chatgpt_test_provider(&server, auth_config, &auth_dir);
        let rx = provider.stream(&chatgpt_stream_request()).await.unwrap();
        let events = collect_provider_events(rx).await;

        assert_eq!(events.len(), 2);
        match &events[0] {
            LlmEvent::TextDelta(text) => assert_eq!(text, "Recovered"),
            other => panic!("expected TextDelta, got {:?}", other),
        }
        assert!(matches!(events[1], LlmEvent::Done { .. }));

        let requests = server.received_requests().await.unwrap();
        let response_requests = requests
            .iter()
            .filter(|request| request.url.path() == "/backend-api/codex/responses")
            .count();
        assert_eq!(response_requests, 2);
    }

    #[tokio::test]
    async fn test_response_api_stream_does_not_retry_server_error_after_output() {
        let server = MockServer::start().await;
        let auth_dir = unique_test_dir("response-failed-after-output");
        write_chatgpt_auth_files(&auth_dir, "chatgpt");

        let mut auth_config = AuthConfig::for_provider("chatgpt").unwrap();
        auth_config.api_base_url = Some(server.uri());
        auth_config.api_path = Some("/backend-api/codex/responses".to_string());

        let failed_after_output_body = concat!(
            "event: response.output_text.delta\n",
            "data: {\"delta\":\"Partial\"}\n\n",
            "event: response.failed\n",
            "data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"code\":\"server_error\",\"message\":\"An error occurred while processing your request.\"}}}\n\n",
        );

        Mock::given(method("POST"))
            .and(path("/backend-api/codex/responses"))
            .and(header("authorization", "Bearer stale-access"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_raw(failed_after_output_body, "text/event-stream"),
            )
            .with_priority(1)
            .mount(&server)
            .await;

        let provider = chatgpt_test_provider(&server, auth_config, &auth_dir);
        let rx = provider.stream(&chatgpt_stream_request()).await.unwrap();
        let events = collect_provider_events(rx).await;

        assert_eq!(events.len(), 2);
        assert!(matches!(&events[0], LlmEvent::TextDelta(text) if text == "Partial"));
        assert!(matches!(&events[1], LlmEvent::Error(message) if message.contains("server_error")));

        let requests = server.received_requests().await.unwrap();
        let response_requests = requests
            .iter()
            .filter(|request| request.url.path() == "/backend-api/codex/responses")
            .count();
        assert_eq!(response_requests, 1);
    }

    #[tokio::test]
    async fn test_chat_completion_stream_closing_before_done_is_error() {
        let (tx, _rx) = mpsc::channel(1);
        let chunk = br#"data: {"choices":[{"delta":{"content":"partial"},"finish_reason":null}]}

"#
        .to_vec();

        let result = process_sse_byte_stream_with_idle_timeout(
            futures::stream::iter([Ok::<_, std::io::Error>(chunk)]),
            &tx,
            &DebugConfig::default(),
            &openai_compat(),
            Duration::from_secs(1),
        )
        .await;

        let error = result.unwrap_err();
        assert!(error.to_string().contains("closed before [DONE]"));
    }

    #[test]
    fn test_parse_retry_after_ms_prefers_millisecond_header() {
        let mut headers = HeaderMap::new();
        headers.insert("retry-after-ms", HeaderValue::from_static("1250"));
        headers.insert(RETRY_AFTER, HeaderValue::from_static("5"));

        assert_eq!(parse_retry_after_ms(&headers), 1250);
    }

    #[test]
    fn test_parse_retry_after_ms_supports_fractional_seconds() {
        let mut headers = HeaderMap::new();
        headers.insert(RETRY_AFTER, HeaderValue::from_static("0.25"));

        assert_eq!(parse_retry_after_ms(&headers), 250);
    }

    #[test]
    fn test_response_event_tool_call_maps_to_tool_use() {
        let mut state = StreamState::new();
        let added_events = parse_response_api_event(
            "response.output_item.added",
            r#"{"item":{"id":"item_1","call_id":"call_1","type":"function_call","name":"read_file"},"response":{"id":"resp_1","model":"gpt-5-codex"}}"#,
            &mut state,
        );
        assert!(added_events.events.is_empty());

        let arg_events = parse_response_api_event(
            "response.function_call_arguments.delta",
            r#"{"item_id":"item_1","delta":"{\"path\":\"/tmp/a.txt\"}","response":{"id":"resp_1","model":"gpt-5-codex"}}"#,
            &mut state,
        );
        assert!(arg_events.events.is_empty());

        let finish_events = parse_response_api_event(
            "response.completed",
            r#"{"response":{"id":"resp_1","model":"gpt-5-codex","stop_reason":"tool_call","usage":{"input_tokens":7,"output_tokens":3}}}"#,
            &mut state,
        );
        assert!(finish_events.terminal);
        assert_eq!(finish_events.events.len(), 2);
        match &finish_events.events[0] {
            LlmEvent::ToolUse {
                id, name, input, ..
            } => {
                assert_eq!(id, "call_1");
                assert_eq!(name, "read_file");
                assert_eq!(input["path"], "/tmp/a.txt");
            }
            other => panic!("expected ToolUse, got {:?}", other),
        }
        match &finish_events.events[1] {
            LlmEvent::Done { stop_reason, usage } => {
                assert_eq!(*stop_reason, StopReason::ToolUse);
                assert_eq!(usage.input_tokens, 7);
                assert_eq!(usage.output_tokens, 3);
            }
            other => panic!("expected Done, got {:?}", other),
        }
    }

    #[test]
    fn test_response_event_completed_without_sse_event_name_defaults_to_end_turn() {
        let mut state = StreamState::new();
        let events = parse_response_api_event(
            "",
            r#"{"type":"response.completed","response":{"id":"resp_1","model":"gpt-5-codex","usage":{"input_tokens":4,"output_tokens":2}}}"#,
            &mut state,
        );

        assert!(events.terminal);
        assert_eq!(events.events.len(), 1);
        match &events.events[0] {
            LlmEvent::Done { stop_reason, usage } => {
                assert_eq!(*stop_reason, StopReason::EndTurn);
                assert_eq!(usage.input_tokens, 4);
                assert_eq!(usage.output_tokens, 2);
            }
            other => panic!("expected Done, got {:?}", other),
        }
    }

    #[test]
    fn test_response_output_item_done_message_emits_text_when_no_delta_arrived() {
        let mut state = StreamState::new();
        let events = parse_response_api_event(
            "response.output_item.done",
            r#"{"item":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Hello from done"}]}}"#,
            &mut state,
        );

        assert!(!events.terminal);
        assert_eq!(events.events.len(), 1);
        match &events.events[0] {
            LlmEvent::TextDelta(text) => assert_eq!(text, "Hello from done"),
            other => panic!("expected TextDelta, got {:?}", other),
        }
    }

    #[test]
    fn test_response_output_item_done_function_call_uses_call_id() {
        let mut state = StreamState::new();
        let tool_item = parse_response_api_event(
            "response.output_item.done",
            r#"{"item":{"id":"fc_item_1","call_id":"call_42","type":"function_call","name":"read_file","arguments":"{\"path\":\"/tmp/a.txt\"}"}}"#,
            &mut state,
        );
        assert!(tool_item.events.is_empty());

        let completed = parse_response_api_event(
            "response.completed",
            r#"{"response":{"id":"resp_1","usage":{"input_tokens":9,"output_tokens":1}}}"#,
            &mut state,
        );

        assert!(completed.terminal);
        assert_eq!(completed.events.len(), 2);
        match &completed.events[0] {
            LlmEvent::ToolUse {
                id, name, input, ..
            } => {
                assert_eq!(id, "call_42");
                assert_eq!(name, "read_file");
                assert_eq!(input["path"], "/tmp/a.txt");
            }
            other => panic!("expected ToolUse, got {:?}", other),
        }
        match &completed.events[1] {
            LlmEvent::Done { stop_reason, .. } => {
                assert_eq!(*stop_reason, StopReason::ToolUse);
            }
            other => panic!("expected Done, got {:?}", other),
        }
    }

    #[test]
    fn test_response_function_call_delta_uses_item_id_when_call_id_missing() {
        let mut state = StreamState::new();
        let arg_events = parse_response_api_event(
            "response.function_call_arguments.delta",
            r#"{"output_index":0,"item_id":"fc_item_9","delta":"{\"path\":\"/tmp/a.txt\"}","response":{"id":"resp_1","model":"gpt-5-codex"}}"#,
            &mut state,
        );
        assert!(arg_events.events.is_empty());

        let completed = parse_response_api_event(
            "response.completed",
            r#"{"response":{"id":"resp_1","stop_reason":"tool_call","usage":{"input_tokens":7,"output_tokens":3}}}"#,
            &mut state,
        );

        assert!(completed.terminal);
        assert_eq!(completed.events.len(), 2);
        match &completed.events[0] {
            LlmEvent::ToolUse { id, input, .. } => {
                assert_eq!(id, "fc_item_9");
                assert_eq!(input["path"], "/tmp/a.txt");
            }
            other => panic!("expected ToolUse, got {:?}", other),
        }
    }

    #[test]
    fn test_response_failed_maps_to_error_and_terminal() {
        let mut state = StreamState::new();
        let events = parse_response_api_event(
            "",
            r#"{"type":"response.failed","response":{"error":{"code":"rate_limit_exceeded","message":"Too many requests"}}}"#,
            &mut state,
        );

        assert!(events.terminal);
        assert_eq!(events.events.len(), 1);
        assert!(matches!(
            events.error,
            Some(ProviderError::RateLimited {
                retry_after_ms: 5000
            })
        ));
        match &events.events[0] {
            LlmEvent::Error(message) => {
                assert_eq!(message, "rate_limit_exceeded: Too many requests");
            }
            other => panic!("expected Error, got {:?}", other),
        }
    }

    #[test]
    fn test_response_failed_server_is_overloaded_maps_to_retryable_error() {
        let mut state = StreamState::new();
        let events = parse_response_api_event(
            "",
            r#"{"type":"response.failed","response":{"error":{"code":"server_is_overloaded","message":"Our servers are currently overloaded. Please try again later."}}}"#,
            &mut state,
        );

        assert!(events.terminal);
        assert!(matches!(
            events.error,
            Some(ProviderError::Api { status: 503, .. })
        ));
        match &events.events[0] {
            LlmEvent::Error(message) => {
                assert_eq!(
                    message,
                    "server_is_overloaded: Our servers are currently overloaded. Please try again later."
                );
            }
            other => panic!("expected Error, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_sse_chunk_accepts_reasoning_alias() {
        let mut state = StreamState::new();
        let events = parse_sse_chunk(
            r#"{"choices":[{"delta":{"reasoning":"think this through"},"finish_reason":null}]}"#,
            &mut state,
            &openai_compat(),
        );

        assert_eq!(events.len(), 1);
        match &events[0] {
            LlmEvent::ThinkingDelta(text) => assert_eq!(text, "think this through"),
            other => panic!("expected ThinkingDelta, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_sse_chunk_accepts_message_reasoning_content() {
        let mut state = StreamState::new();
        let events = parse_sse_chunk(
            r#"{"choices":[{"delta":{},"message":{"reasoning_content":"tool reasoning"},"finish_reason":"tool_calls"}]}"#,
            &mut state,
            &openai_compat(),
        );

        assert!(
            events.iter().any(
                |event| matches!(event, LlmEvent::ThinkingDelta(text) if text == "tool reasoning")
            ),
            "expected message.reasoning_content to produce ThinkingDelta, got {events:?}"
        );
    }

    #[test]
    fn test_parse_sse_chunk_accepts_message_tool_calls() {
        let mut state = StreamState::new();
        let events = parse_sse_chunk(
            r#"{"choices":[{"delta":{},"message":{"tool_calls":[{"id":"call_1","type":"function","function":{"name":"read_file","arguments":"{\"path\":\"README.md\"}"}}]},"finish_reason":"tool_calls"}]}"#,
            &mut state,
            &openai_compat(),
        );

        assert!(
            events.iter().any(|event| matches!(
                event,
                LlmEvent::ToolUse {
                    id, name, input, ..
                }
                    if id == "call_1" && name == "read_file" && input["path"] == "README.md"
            )),
            "expected message.tool_calls to produce ToolUse, got {events:?}"
        );
    }

    #[test]
    fn test_parse_sse_chunk_filters_dsml_tool_call_sentinel_only() {
        let mut state = StreamState::new();
        let json = json!({
            "choices": [{
                "delta": {
                    "content": "\n\n<\u{ff5c}DSML\u{ff5c}tool_calls"
                },
                "finish_reason": null
            }]
        });
        let events = parse_openai_chunk_json(&json, &mut state, &openai_compat());

        assert!(
            events.is_empty(),
            "expected no visible text, got {events:?}"
        );
    }

    #[test]
    fn test_parse_sse_chunk_filters_dsml_tool_call_sentinel_after_visible_text() {
        let mut state = StreamState::new();
        let json = json!({
            "choices": [{
                "delta": {
                    "content": "找到位置了。\n\n<\u{ff5c}DSML\u{ff5c}tool_calls"
                },
                "finish_reason": null
            }]
        });
        let events = parse_openai_chunk_json(&json, &mut state, &openai_compat());

        assert_eq!(events.len(), 1);
        match &events[0] {
            LlmEvent::TextDelta(text) => assert_eq!(text, "找到位置了。"),
            other => panic!("expected TextDelta, got {other:?}"),
        }
    }

    #[test]
    fn test_build_messages_assistant_tool_call_with_reasoning_includes_content() {
        let messages = OpenAIProvider::build_messages(
            &[
                Message::new(
                    Role::Assistant,
                    vec![
                        ContentBlock::Thinking {
                            thinking: "tool reasoning".into(),
                        },
                        ContentBlock::ToolUse {
                            id: "call_1".into(),
                            name: "read_file".into(),
                            input: json!({"path": "README.md"}),
                            extra: None,
                        },
                    ],
                ),
                Message::new(
                    Role::User,
                    vec![ContentBlock::ToolResult {
                        tool_use_id: "call_1".into(),
                        content: "readme content".into(),
                        is_error: false,
                    }],
                ),
            ],
            "",
            &openai_compat(),
        );

        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["reasoning_content"], "tool reasoning");
        assert_eq!(messages[0]["content"], "");
        assert_eq!(messages[0]["tool_calls"][0]["id"], "call_1");
    }

    #[test]
    fn test_build_messages_synthesizes_missing_tool_call_reasoning_when_enabled() {
        let mut compat = openai_compat();
        compat.synthesize_missing_tool_call_reasoning_content = Some(true);

        let messages = OpenAIProvider::build_messages(
            &[
                Message::new(
                    Role::Assistant,
                    vec![ContentBlock::ToolUse {
                        id: "call_1".into(),
                        name: "read_file".into(),
                        input: json!({"path": "README.md"}),
                        extra: None,
                    }],
                ),
                Message::new(
                    Role::User,
                    vec![ContentBlock::ToolResult {
                        tool_use_id: "call_1".into(),
                        content: "readme content".into(),
                        is_error: false,
                    }],
                ),
            ],
            "",
            &compat,
        );

        assert_eq!(
            messages[0]["reasoning_content"],
            SYNTHETIC_TOOL_CALL_REASONING_CONTENT
        );
        assert_eq!(messages[0]["content"], "");
        assert_eq!(messages[0]["tool_calls"][0]["id"], "call_1");
    }

    #[test]
    fn test_build_messages_does_not_synthesize_missing_tool_call_reasoning_by_default() {
        let messages = OpenAIProvider::build_messages(
            &[
                Message::new(
                    Role::Assistant,
                    vec![ContentBlock::ToolUse {
                        id: "call_1".into(),
                        name: "read_file".into(),
                        input: json!({"path": "README.md"}),
                        extra: None,
                    }],
                ),
                Message::new(
                    Role::User,
                    vec![ContentBlock::ToolResult {
                        tool_use_id: "call_1".into(),
                        content: "readme content".into(),
                        is_error: false,
                    }],
                ),
            ],
            "",
            &openai_compat(),
        );

        assert!(messages[0].get("reasoning_content").is_none());
        assert_eq!(messages[0]["tool_calls"][0]["id"], "call_1");
    }

    #[test]
    fn test_build_messages_strips_assistant_dsml_tool_call_sentinel() {
        let messages = OpenAIProvider::build_messages(
            &[Message::new(
                Role::Assistant,
                vec![ContentBlock::Text {
                    text: "找到位置了。\n\n<\u{ff5c}DSML\u{ff5c}tool_calls".into(),
                }],
            )],
            "",
            &openai_compat(),
        );

        assert_eq!(messages[0]["content"], "找到位置了。");
    }

    #[test]
    fn test_merge_consecutive_assistant_preserves_reasoning_content() {
        let mut messages = vec![
            json!({
                "role": "assistant",
                "reasoning_content": "first ",
                "content": "hello"
            }),
            json!({
                "role": "assistant",
                "reasoning_content": "second",
                "tool_calls": [{
                    "id": "call_1",
                    "type": "function",
                    "function": {
                        "name": "read_file",
                        "arguments": "{\"path\":\"README.md\"}"
                    }
                }]
            }),
        ];

        merge_consecutive_assistant(&mut messages);

        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["reasoning_content"], "first second");
        assert_eq!(messages[0]["content"], "hello");
        assert_eq!(messages[0]["tool_calls"][0]["id"], "call_1");
    }

    // --- clean_orphan_tool_calls ---

    #[test]
    fn test_clean_orphan_tool_calls_enabled() {
        let messages = vec![
            Message::new(
                Role::Assistant,
                vec![
                    ContentBlock::ToolUse {
                        id: "tc1".into(),
                        name: "bash".into(),
                        input: json!({}),
                        extra: None,
                    },
                    ContentBlock::ToolUse {
                        id: "tc2".into(),
                        name: "read".into(),
                        input: json!({}),
                        extra: None,
                    },
                ],
            ),
            Message::new(
                Role::Tool,
                vec![ContentBlock::ToolResult {
                    tool_use_id: "tc1".into(),
                    content: "ok".into(),
                    is_error: false,
                }],
            ),
            // tc2 has no result -> orphan
        ];
        let result = OpenAIProvider::build_messages(&messages, "", &openai_compat());
        let assistant = result.iter().find(|m| m["role"] == "assistant").unwrap();
        let tcs = assistant["tool_calls"].as_array().unwrap();
        assert_eq!(tcs.len(), 1);
        assert_eq!(tcs[0]["id"], "tc1");
    }

    #[test]
    fn test_clean_orphan_tool_calls_disabled() {
        let messages = vec![
            Message::new(
                Role::Assistant,
                vec![
                    ContentBlock::ToolUse {
                        id: "tc1".into(),
                        name: "bash".into(),
                        input: json!({}),
                        extra: None,
                    },
                    ContentBlock::ToolUse {
                        id: "tc2".into(),
                        name: "read".into(),
                        input: json!({}),
                        extra: None,
                    },
                ],
            ),
            Message::new(
                Role::Tool,
                vec![ContentBlock::ToolResult {
                    tool_use_id: "tc1".into(),
                    content: "ok".into(),
                    is_error: false,
                }],
            ),
        ];
        let result = OpenAIProvider::build_messages(&messages, "", &no_compat());
        let assistant = result.iter().find(|m| m["role"] == "assistant").unwrap();
        let tcs = assistant["tool_calls"].as_array().unwrap();
        assert_eq!(tcs.len(), 2);
    }

    // --- dedup_tool_results ---

    #[test]
    fn test_dedup_tool_results_enabled() {
        let messages = vec![
            Message::new(
                Role::Assistant,
                vec![ContentBlock::ToolUse {
                    id: "tc1".into(),
                    name: "bash".into(),
                    input: json!({}),
                    extra: None,
                }],
            ),
            Message::new(
                Role::Tool,
                vec![ContentBlock::ToolResult {
                    tool_use_id: "tc1".into(),
                    content: "first".into(),
                    is_error: false,
                }],
            ),
            Message::new(
                Role::Tool,
                vec![ContentBlock::ToolResult {
                    tool_use_id: "tc1".into(),
                    content: "second".into(),
                    is_error: false,
                }],
            ),
        ];
        let result = OpenAIProvider::build_messages(&messages, "", &openai_compat());
        let tool_msgs: Vec<_> = result.iter().filter(|m| m["role"] == "tool").collect();
        assert_eq!(tool_msgs.len(), 1);
        assert_eq!(tool_msgs[0]["content"], "second");
    }

    // --- usage token parsing ---

    #[test]
    fn test_usage_from_trailing_chunk() {
        // OpenAI sends usage in a trailing chunk where choices:[] — the Done
        // event must carry the token counts from that chunk, not zeros.
        let mut state = StreamState::new();

        // chunk 1: finish_reason + text delta, no usage
        let chunk1 = r#"{"choices":[{"delta":{"content":"hi"},"finish_reason":"stop"}]}"#;
        let events = parse_sse_chunk(chunk1, &mut state, &openai_compat());
        // TextDelta is emitted immediately; Done is deferred.
        assert!(
            events.iter().all(|e| !matches!(e, LlmEvent::Done { .. })),
            "Done should be deferred, not emitted with finish_reason chunk"
        );
        assert!(state.pending_done.is_some());

        // chunk 2: trailing usage-only chunk (choices:[])
        let chunk2 = r#"{"choices":[],"usage":{"prompt_tokens":10,"completion_tokens":5}}"#;
        let events2 = parse_sse_chunk(chunk2, &mut state, &openai_compat());
        assert!(events2.is_empty());
        assert_eq!(state.input_tokens, 10);
        assert_eq!(state.output_tokens, 5);

        // [DONE] — flush with final counts
        let done = state.flush_done().expect("pending_done should be Some");
        match done {
            LlmEvent::Done { stop_reason, usage } => {
                assert_eq!(stop_reason, StopReason::EndTurn);
                assert_eq!(usage.input_tokens, 10);
                assert_eq!(usage.output_tokens, 5);
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }

    #[test]
    fn test_usage_in_finish_chunk() {
        // Some providers/models include usage in the same chunk as finish_reason.
        // Counts should still be correct after flush.
        let mut state = StreamState::new();

        // No text delta here, only finish_reason + usage in the same chunk.
        let chunk = r#"{"choices":[{"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":8,"completion_tokens":3}}"#;
        let events = parse_sse_chunk(chunk, &mut state, &openai_compat());
        assert!(
            events.iter().all(|e| !matches!(e, LlmEvent::Done { .. })),
            "Done should be deferred even when usage is in the finish chunk"
        );
        assert_eq!(state.output_tokens, 3);

        let done = state.flush_done().unwrap();
        match done {
            LlmEvent::Done { usage, .. } => {
                assert_eq!(usage.output_tokens, 3);
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }

    #[test]
    fn test_build_tools_deferred_has_empty_parameters() {
        let tools = vec![
            ToolDef {
                name: "Read".into(),
                description: "Read a file".into(),
                input_schema: json!({"type": "object", "properties": {"path": {"type": "string"}}}),
                deferred: false,
            },
            ToolDef {
                name: "SpawnTool".into(),
                description: "Spawn sub-agents".into(),
                input_schema: json!({"type": "object", "properties": {"agents": {"type": "array"}}}),
                deferred: true,
            },
        ];
        let result = OpenAIProvider::build_tools(&tools);

        // Core tool has full parameters
        let read_params = &result[0]["function"]["parameters"];
        assert!(read_params["properties"].get("path").is_some());

        // Deferred tool has empty parameters and modified description
        let spawn_params = &result[1]["function"]["parameters"];
        assert!(spawn_params["properties"].as_object().unwrap().is_empty());
        let spawn_desc = result[1]["function"]["description"].as_str().unwrap();
        assert!(spawn_desc.contains("ToolSearch"));
    }
}
