use std::future::Future;
use std::time::Duration;

use reqwest::header::HeaderMap;
use serde_json::Value;

use crate::error::ProviderError;

/// Retry a fallible async operation with exponential backoff
pub async fn with_retry<F, Fut, T>(max_retries: u32, f: F) -> Result<T, ProviderError>
where
    F: Fn() -> Fut,
    Fut: Future<Output = Result<T, ProviderError>>,
{
    let mut backoff = Duration::from_secs(1);
    for attempt in 0..=max_retries {
        match f().await {
            Ok(val) => return Ok(val),
            Err(e) if e.is_retryable() && attempt < max_retries => {
                tracing::warn!(attempt = attempt + 1, max_retries, error = %e, "retrying request");
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(Duration::from_secs(30));
            }
            Err(e) => return Err(e),
        }
    }
    unreachable!()
}

pub const MAX_STREAM_RETRIES: u32 = 2;
pub const MAX_INITIAL_CONNECT_RETRIES: u32 = 2;
const INITIAL_HTTP_5XX_RETRY_BACKOFFS: [Duration; 5] = [
    Duration::from_secs(1),
    Duration::from_secs(5),
    Duration::from_secs(10),
    Duration::from_secs(30),
    Duration::from_secs(60),
];
const MAX_BACKOFF: Duration = Duration::from_secs(15);
const INITIAL_CONNECT_BACKOFF: Duration = Duration::from_millis(300);
const MAX_INITIAL_CONNECT_BACKOFF: Duration = Duration::from_secs(2);

/// Retry initial request failures that occur before an HTTP response exists.
/// HTTP status errors and rate limits are intentionally not retried here.
pub async fn with_initial_connect_retry<F, Fut, T>(f: F) -> Result<T, ProviderError>
where
    F: Fn() -> Fut,
    Fut: Future<Output = Result<T, ProviderError>>,
{
    let mut backoff = INITIAL_CONNECT_BACKOFF;
    for attempt in 0..=MAX_INITIAL_CONNECT_RETRIES {
        match f().await {
            Ok(val) => return Ok(val),
            Err(e) if is_initial_connect_error(&e) && attempt < MAX_INITIAL_CONNECT_RETRIES => {
                tracing::warn!(
                    attempt = attempt + 1,
                    max_retries = MAX_INITIAL_CONNECT_RETRIES,
                    error = %e,
                    "retrying initial provider request after connect failure"
                );
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(MAX_INITIAL_CONNECT_BACKOFF);
            }
            Err(e) => return Err(e),
        }
    }
    unreachable!()
}

fn is_initial_connect_error(error: &ProviderError) -> bool {
    match error {
        ProviderError::Http(err) => err.is_connect(),
        ProviderError::Connection(_) => true,
        _ => false,
    }
}

/// Retry transient provider-side HTTP failures before stream consumption starts.
pub(crate) async fn with_initial_http_5xx_retry<F, Fut, T>(f: F) -> Result<T, ProviderError>
where
    F: Fn() -> Fut,
    Fut: Future<Output = Result<T, ProviderError>>,
{
    let max_retries = INITIAL_HTTP_5XX_RETRY_BACKOFFS.len();
    for (attempt, backoff) in INITIAL_HTTP_5XX_RETRY_BACKOFFS.iter().enumerate() {
        match f().await {
            Ok(val) => return Ok(val),
            Err(e) => match initial_http_5xx_status(&e) {
                Some(status) => {
                    tracing::warn!(
                        attempt = attempt + 1,
                        max_retries,
                        status,
                        "retrying initial provider request after server error"
                    );
                    tokio::time::sleep(*backoff).await;
                }
                _ => return Err(e),
            },
        }
    }
    f().await
}

fn initial_http_5xx_status(error: &ProviderError) -> Option<u16> {
    match error {
        ProviderError::Api { status, .. } if (500..=599).contains(status) => Some(*status),
        _ => None,
    }
}

/// Send an HTTP request and check status, returning the response on success.
/// Used by provider-specific retry loops to avoid duplicating request logic.
pub async fn send_and_check(
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
        .map_err(|e| ProviderError::Connection(e.to_string()))?;

    let status = response.status();
    if !status.is_success() {
        let body_text = response.text().await.unwrap_or_default();
        return Err(ProviderError::Api {
            status: status.as_u16(),
            message: body_text,
        });
    }

    Ok(response)
}

/// Sleep with exponential backoff and log the retry attempt.
/// Returns the next backoff duration.
pub async fn backoff_sleep(attempt: u32, current_backoff: Duration) -> Duration {
    tracing::warn!(
        attempt,
        max = MAX_STREAM_RETRIES,
        "retrying stream after mid-stream disconnect"
    );
    tokio::time::sleep(current_backoff).await;
    (current_backoff * 2).min(MAX_BACKOFF)
}

#[cfg(test)]
#[path = "retry_test.rs"]
mod retry_test;
