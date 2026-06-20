use reqwest::header::{HeaderMap, RETRY_AFTER};

#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("API error {status}: {message}")]
    Api { status: u16, message: String },
    #[error("API error {status}: {message}")]
    ApiSignal {
        status: u16,
        message: String,
        raw_code: Option<String>,
        raw_type: Option<String>,
        retry_after_ms: Option<u64>,
    },
    #[error("SSE parse error: {0}")]
    Parse(String),
    #[error("Rate limited, retry after {retry_after_ms}ms")]
    RateLimited { retry_after_ms: u64 },
    #[error("Prompt too long: {0}")]
    PromptTooLong(String),
    #[error("Connection error: {0}")]
    Connection(String),
}

impl ProviderError {
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            ProviderError::RateLimited { .. } | ProviderError::Connection(_)
        )
    }
}

pub fn retry_after_ms_from_headers(headers: &HeaderMap) -> Option<u64> {
    headers
        .get(RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .map(|seconds| seconds.saturating_mul(1000))
}

pub fn provider_api_error(
    status: u16,
    message: impl Into<String>,
    retry_after_ms: Option<u64>,
) -> ProviderError {
    ProviderError::ApiSignal {
        status,
        message: message.into(),
        raw_code: None,
        raw_type: None,
        retry_after_ms: retry_after_ms.or_else(|| (status == 429).then_some(5000)),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderRawSignals {
    pub provider: String,
    pub model: Option<String>,
    pub status: Option<u16>,
    pub raw_code: Option<String>,
    pub raw_type: Option<String>,
    pub message: Option<String>,
    pub request_id: Option<String>,
    pub retry_after_ms: Option<u64>,
    pub source: ProviderRawSignalSource,
    pub phase: ProviderFailurePhase,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderRawSignalSource {
    Auth,
    RequestBuild,
    Http,
    Sse,
    Json,
    Transport,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderFailurePhase {
    BeforeFirstDelta,
    AfterFirstDelta,
    AfterToolCall,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderFailure {
    pub kind: ProviderFailureKind,
    pub meta: ProviderFailureMeta,
}

impl ProviderFailure {
    pub fn tool_call_invalid(
        provider: impl Into<String>,
        model: Option<String>,
        reason: impl Into<String>,
        phase: ProviderFailurePhase,
    ) -> ProviderFailure {
        let reason = summarize_raw_value(reason.into());
        ProviderFailure {
            kind: ProviderFailureKind::ToolCall(ToolCallFailure {
                reason: ToolCallFailureReason::InvalidProviderToolCall,
                detail: Some(reason.clone()),
            }),
            meta: ProviderFailureMeta {
                provider: provider.into(),
                model,
                status: None,
                request_id: None,
                retry_after_ms: None,
                phase,
                raw: RawProviderErrorSummary {
                    raw_code: None,
                    raw_type: None,
                    message: Some(summarize_raw_value(reason)),
                },
            },
        }
    }
}

impl std::fmt::Display for ProviderFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.meta.raw.message.as_deref() {
            Some(message) => write!(
                f,
                "{:?} from {} during {:?}: {}",
                self.kind, self.meta.provider, self.meta.phase, message
            ),
            None => write!(
                f,
                "{:?} from {} during {:?}",
                self.kind, self.meta.provider, self.meta.phase
            ),
        }
    }
}

impl std::error::Error for ProviderFailure {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderFailureMeta {
    pub provider: String,
    pub model: Option<String>,
    pub status: Option<u16>,
    pub request_id: Option<String>,
    pub retry_after_ms: Option<u64>,
    pub phase: ProviderFailurePhase,
    pub raw: RawProviderErrorSummary,
}

impl ProviderFailureMeta {
    fn from_raw(raw: ProviderRawSignals) -> ProviderFailureMeta {
        ProviderFailureMeta {
            provider: raw.provider,
            model: raw.model,
            status: raw.status,
            request_id: raw.request_id,
            retry_after_ms: raw.retry_after_ms,
            phase: raw.phase,
            raw: RawProviderErrorSummary {
                raw_code: raw.raw_code.map(summarize_raw_value),
                raw_type: raw.raw_type.map(summarize_raw_value),
                message: raw.message.map(summarize_raw_value),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawProviderErrorSummary {
    pub raw_code: Option<String>,
    pub raw_type: Option<String>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderFailureKind {
    CredentialMissing,
    AuthFailed,
    PermissionDenied,
    BillingRequired,
    QuotaExceeded,
    RateLimited,
    ModelNotFound,
    ModelUnsupported,
    ModelUnavailable,
    EndpointNotFound,
    ContextTooLarge,
    ToolSchema(ToolSchemaFailure),
    ToolCall(ToolCallFailure),
    ContentBlocked,
    Timeout,
    Transport,
    Server,
    InvalidRequest(InvalidProviderRequestFailure),
    ResponseParse,
    EmptyResponse,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolSchemaFailure {
    pub source: ToolSchemaSource,
    pub reason: ToolSchemaFailureReason,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolSchemaSource {
    AionrsRequest,
    ProviderResponse,
    McpCapability,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolSchemaFailureReason {
    InvalidSchema,
    UnsupportedSchema,
    BadRequest,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCallFailure {
    pub reason: ToolCallFailureReason,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolCallFailureReason {
    InvalidProviderToolCall,
    MissingName,
    MissingId,
    InvalidArguments,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidProviderRequestFailure {
    pub source: InvalidProviderRequestSource,
    pub reason: InvalidProviderRequestReason,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvalidProviderRequestSource {
    AionrsRequestBuilder,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvalidProviderRequestReason {
    BadRequest,
    Unknown,
}

pub fn classify_provider_failure(raw: ProviderRawSignals) -> ProviderFailure {
    let kind = classify_provider_failure_kind(&raw);
    let meta = ProviderFailureMeta::from_raw(raw);
    ProviderFailure { kind, meta }
}

pub fn provider_failure_from_error(
    provider: impl Into<String>,
    model: Option<String>,
    error: ProviderError,
    phase: ProviderFailurePhase,
) -> ProviderFailure {
    let provider = provider.into();
    let raw = match error {
        ProviderError::Http(error) => {
            let status = error.status().map(|status| status.as_u16());
            ProviderRawSignals {
                provider,
                model,
                status,
                raw_code: None,
                raw_type: None,
                message: Some(error.to_string()),
                request_id: None,
                retry_after_ms: None,
                source: if status.is_none() || error.is_connect() || error.is_timeout() {
                    ProviderRawSignalSource::Transport
                } else {
                    ProviderRawSignalSource::Http
                },
                phase,
            }
        }
        ProviderError::Api { status, message } => ProviderRawSignals {
            provider,
            model,
            status: Some(status),
            raw_code: None,
            raw_type: None,
            message: Some(message),
            request_id: None,
            retry_after_ms: None,
            source: ProviderRawSignalSource::Http,
            phase,
        },
        ProviderError::ApiSignal {
            status,
            message,
            raw_code,
            raw_type,
            retry_after_ms,
        } => ProviderRawSignals {
            provider,
            model,
            status: Some(status),
            raw_code,
            raw_type,
            message: Some(message),
            request_id: None,
            retry_after_ms,
            source: ProviderRawSignalSource::Http,
            phase,
        },
        ProviderError::Parse(message) => ProviderRawSignals {
            provider,
            model,
            status: None,
            raw_code: None,
            raw_type: None,
            message: Some(message),
            request_id: None,
            retry_after_ms: None,
            source: ProviderRawSignalSource::Sse,
            phase,
        },
        ProviderError::RateLimited { retry_after_ms } => ProviderRawSignals {
            provider,
            model,
            status: Some(429),
            raw_code: None,
            raw_type: None,
            message: None,
            request_id: None,
            retry_after_ms: Some(retry_after_ms),
            source: ProviderRawSignalSource::Http,
            phase,
        },
        ProviderError::PromptTooLong(message) => ProviderRawSignals {
            provider,
            model,
            status: Some(400),
            raw_code: Some("prompt_too_long".to_string()),
            raw_type: None,
            message: Some(message),
            request_id: None,
            retry_after_ms: None,
            source: ProviderRawSignalSource::Http,
            phase,
        },
        ProviderError::Connection(message) => ProviderRawSignals {
            provider,
            model,
            status: None,
            raw_code: None,
            raw_type: None,
            message: Some(message),
            request_id: None,
            retry_after_ms: None,
            source: ProviderRawSignalSource::Transport,
            phase,
        },
    };

    classify_provider_failure(raw)
}

fn classify_provider_failure_kind(raw: &ProviderRawSignals) -> ProviderFailureKind {
    let status = raw.status;
    let text = signal_text(raw);

    if status == Some(401) || has_any(&text, &auth_invalid_signals()) {
        return ProviderFailureKind::AuthFailed;
    }

    if raw.source == ProviderRawSignalSource::Auth {
        return ProviderFailureKind::CredentialMissing;
    }

    if has_any(&text, &["permission_error", "permission denied"]) {
        return ProviderFailureKind::PermissionDenied;
    }

    if has_any(
        &text,
        &[
            "rate_limit_error",
            "rate_limit",
            "rate limit",
            "too many requests",
        ],
    ) {
        return ProviderFailureKind::RateLimited;
    }

    if has_any(&text, &["overloaded_error", "overloaded"]) {
        return ProviderFailureKind::ModelUnavailable;
    }

    if has_any(&text, &["api_error", "server_error"]) {
        return ProviderFailureKind::Server;
    }

    if has_any(
        &text,
        &[
            "context length",
            "context_length",
            "maximum context",
            "maximum_context",
            "too many tokens",
            "too_many_tokens",
            "prompt too long",
            "prompt_too_long",
        ],
    ) {
        return ProviderFailureKind::ContextTooLarge;
    }

    if has_any(
        &text,
        &["insufficient_quota", "quota exceeded", "quota_exceeded"],
    ) {
        return ProviderFailureKind::QuotaExceeded;
    }

    if has_any(
        &text,
        &["content_filter", "content blocked", "content_blocked"],
    ) {
        return ProviderFailureKind::ContentBlocked;
    }

    if has_any(&text, &["timeout", "timed out"]) {
        return ProviderFailureKind::Timeout;
    }

    match status {
        Some(403) => return ProviderFailureKind::PermissionDenied,
        Some(402) => return ProviderFailureKind::BillingRequired,
        Some(429) => return ProviderFailureKind::RateLimited,
        Some(500..=599) => return ProviderFailureKind::Server,
        Some(404) if has_model_not_found_signal(&text) => {
            return ProviderFailureKind::ModelNotFound;
        }
        Some(404) => return ProviderFailureKind::EndpointNotFound,
        _ => {}
    }

    if matches!(
        raw.source,
        ProviderRawSignalSource::Sse | ProviderRawSignalSource::Json
    ) {
        return ProviderFailureKind::ResponseParse;
    }

    if raw.source == ProviderRawSignalSource::RequestBuild {
        return ProviderFailureKind::InvalidRequest(InvalidProviderRequestFailure {
            source: InvalidProviderRequestSource::AionrsRequestBuilder,
            reason: InvalidProviderRequestReason::BadRequest,
            detail: raw.message.clone().map(summarize_raw_value),
        });
    }

    if status == Some(400) {
        return ProviderFailureKind::InvalidRequest(InvalidProviderRequestFailure {
            source: InvalidProviderRequestSource::Unknown,
            reason: InvalidProviderRequestReason::BadRequest,
            detail: raw.message.clone().map(summarize_raw_value),
        });
    }

    if raw.source == ProviderRawSignalSource::Transport {
        return ProviderFailureKind::Transport;
    }

    ProviderFailureKind::Unknown
}

fn signal_text(raw: &ProviderRawSignals) -> String {
    let mut parts = Vec::new();
    if let Some(raw_code) = raw.raw_code.as_deref() {
        parts.push(raw_code);
    }
    if let Some(raw_type) = raw.raw_type.as_deref() {
        parts.push(raw_type);
    }
    if let Some(message) = raw.message.as_deref() {
        parts.push(message);
    }
    parts.join(" ").to_ascii_lowercase()
}

fn has_any(text: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| text.contains(needle))
}

fn has_model_not_found_signal(text: &str) -> bool {
    has_any(
        text,
        &[
            "model_not_found",
            "model-not-found",
            "model not found",
            "model",
            "模型不存在",
            "模型未找到",
        ],
    )
}

fn auth_invalid_signals() -> &'static [&'static str] {
    &[
        "invalid_api_key",
        "invalid api key",
        "incorrect api key",
        "authentication failed",
        "authentication_error",
        "auth invalid",
        "invalid auth",
        "invalid token",
        "unauthorized",
    ]
}

fn summarize_raw_value(value: String) -> String {
    const MAX_RAW_SUMMARY_CHARS: usize = 512;

    let value = redact_known_secret_shapes(value);
    if value.chars().count() <= MAX_RAW_SUMMARY_CHARS {
        return value;
    }

    value
        .chars()
        .take(MAX_RAW_SUMMARY_CHARS)
        .collect::<String>()
}

fn redact_known_secret_shapes(value: String) -> String {
    value
        .split_whitespace()
        .map(|part| {
            if looks_like_provider_secret(part) {
                "[redacted]"
            } else {
                part
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn looks_like_provider_secret(value: &str) -> bool {
    let trimmed = value.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_');
    let lowered = trimmed.to_ascii_lowercase();

    (lowered.starts_with("sk-") && trimmed.len() >= 20)
        || (lowered.starts_with("ya29.") && trimmed.len() >= 20)
        || looks_like_aws_access_key_id(trimmed)
}

fn looks_like_aws_access_key_id(value: &str) -> bool {
    value.len() == 20
        && (value.starts_with("AKIA") || value.starts_with("ASIA"))
        && value
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
}

impl From<ProviderError> for ProviderFailure {
    fn from(error: ProviderError) -> ProviderFailure {
        provider_failure_from_error(
            "unknown",
            None,
            error,
            ProviderFailurePhase::BeforeFirstDelta,
        )
    }
}

#[cfg(test)]
mod retryable_tests {
    use super::*;

    // F1-11
    #[test]
    fn test_api_400_not_retryable() {
        assert!(
            !ProviderError::Api {
                status: 400,
                message: "empty name".into(),
            }
            .is_retryable()
        );
        assert!(
            ProviderError::RateLimited {
                retry_after_ms: 1000
            }
            .is_retryable()
        );
        assert!(ProviderError::Connection("x".into()).is_retryable());
    }
}
