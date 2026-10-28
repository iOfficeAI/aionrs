use serde_json::Value;

#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("API error {status}: {message}")]
    Api { status: u16, message: String },
    #[error("SSE parse error: {0}")]
    Parse(String),
    // Display intentionally omits `body` — it may contain provider response
    // payload (potentially sensitive) and would leak into logs via
    // `tracing::error!("{err}")`. Consumers that need the body must pattern
    // match on the variant explicitly.
    #[error("Rate limited, retry after {retry_after_ms}ms")]
    RateLimited { retry_after_ms: u64, body: Option<String> },
    #[error("Prompt too long: {0}")]
    PromptTooLong(String),
    #[error("Connection error: {0}")]
    Connection(String),
}

impl ProviderError {
    pub fn is_retryable(&self) -> bool {
        matches!(self, ProviderError::RateLimited { .. } | ProviderError::Connection(_))
    }
}

/// HTTP statuses providers use to reject an oversized request. OpenAI-compatible
/// servers (sglang, vLLM, llama.cpp) answer 400; some gateways use 413 or 422.
const PROMPT_TOO_LONG_STATUSES: [u16; 3] = [400, 413, 422];

/// Case-insensitive substrings that identify a window-overflow rejection. Kept
/// narrow on purpose: these messages must not swallow ordinary 400s such as
/// "invalid request" or a malformed tool schema.
const PROMPT_TOO_LONG_MARKERS: [&str; 9] = [
    "maximum context length",
    "context length exceeded",
    "context window exceeds",
    "exceeds the model's maximum context",
    "prompt is too long",
    "input is too long",
    "reduce the number of tokens",
    "requested token count exceeds",
    "too many tokens",
];

/// Classify an HTTP error as a context-window overflow.
///
/// `PromptTooLong` is what lets the engine recover — autocompact trims the
/// conversation and retries, and the main turn can compact instead of failing —
/// so an OpenAI-compatible "your prompt does not fit" 400 must not stay a plain
/// `Api { status: 400 }`: that variant is neither retryable nor recognizable,
/// and autocompact surfaced it verbatim as `Autocompact failed`.
pub(crate) fn prompt_too_long(status: u16, message: &str) -> Option<ProviderError> {
    if !PROMPT_TOO_LONG_STATUSES.contains(&status) {
        return None;
    }
    let lower = message.to_ascii_lowercase();
    if PROMPT_TOO_LONG_MARKERS.iter().any(|marker| lower.contains(marker)) {
        return Some(ProviderError::PromptTooLong(message.trim().to_owned()));
    }
    None
}

/// Extract the model's context window (in tokens) from a provider error message.
///
/// Providers phrase window overflow differently, for example:
/// - `This model's maximum context length is 8192 tokens.`
/// - `Requested token count exceeds the model's maximum context length of 131077 tokens.`
/// - `prompt is too long: 213000 tokens > 200000 maximum`
///
/// Returns `None` when no plausible limit can be read. Values outside a sane
/// range are ignored so an unrelated number in the message cannot corrupt the
/// window.
pub fn parse_context_limit_hint(message: &str) -> Option<usize> {
    let lower = message.to_ascii_lowercase();

    const MARKERS: [&str; 8] = [
        "maximum context length is",
        "maximum context length of",
        "max context length is",
        "context length is",
        "context length of",
        "context window is",
        "context window of",
        "tokens >",
    ];

    for marker in MARKERS {
        if let Some(position) = lower.find(marker) {
            let rest = &lower[position + marker.len()..];
            if let Some(limit) = first_plausible_number(rest) {
                return Some(limit);
            }
        }
    }
    None
}

/// Return the first integer in `text` that could plausibly be a context window.
fn first_plausible_number(text: &str) -> Option<usize> {
    const MIN_PLAUSIBLE: usize = 256;
    const MAX_PLAUSIBLE: usize = 10_000_000;

    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if !bytes[index].is_ascii_digit() {
            index += 1;
            continue;
        }

        let start = index;
        while index < bytes.len() && (bytes[index].is_ascii_digit() || matches!(bytes[index], b'_' | b',')) {
            index += 1;
        }
        let digits: String = text[start..index].chars().filter(char::is_ascii_digit).collect();
        if let Ok(value) = digits.parse::<usize>()
            && (MIN_PLAUSIBLE..=MAX_PLAUSIBLE).contains(&value)
        {
            return Some(value);
        }
    }
    None
}

/// Map a provider JSON error payload to a `ProviderError`.
///
/// Covers error bodies delivered with a successful HTTP status: whole-body
/// JSON errors on non-streaming responses and `data: {"error": ...}` frames
/// embedded in SSE streams. Returns `None` when the body does not look like
/// an error payload.
pub(crate) fn provider_error_from_json_body(body: &Value, body_bytes: &[u8]) -> Option<ProviderError> {
    // Some gateways include `"error": null` in perfectly normal responses;
    // treat a null error field the same as an absent one.
    let error_field = body.get("error").filter(|error| !error.is_null());
    let error = error_field.unwrap_or(body);
    let status = [
        error.get("code"),
        error.get("status"),
        body.get("code"),
        body.get("status"),
    ]
    .into_iter()
    .flatten()
    .find_map(json_http_status_code);
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .or_else(|| error.as_str())
        .or_else(|| body.get("message").and_then(Value::as_str))
        .map(str::trim)
        .filter(|message| !message.is_empty())
        .unwrap_or("Provider returned a JSON error response without a message")
        .to_string();

    match status {
        Some(429) => Some(ProviderError::RateLimited {
            retry_after_ms: 5000,
            body: (!body_bytes.is_empty()).then(|| String::from_utf8_lossy(body_bytes).into_owned()),
        }),
        Some(status) => prompt_too_long(status, &message).or(Some(ProviderError::Api { status, message })),
        None if error_field.is_some() => Some(ProviderError::Parse(format!(
            "Provider returned a JSON error response without an HTTP status: {message}"
        ))),
        None => None,
    }
}

fn json_http_status_code(value: &Value) -> Option<u16> {
    value
        .as_u64()
        .and_then(|status| u16::try_from(status).ok())
        .or_else(|| value.as_str().and_then(|status| status.parse().ok()))
        .filter(|status| (400..=599).contains(status))
}

#[cfg(test)]
#[path = "error_test.rs"]
mod error_test;
