use std::future::Future;
use std::time::Duration;

use rand::Rng;
use reqwest::header::HeaderMap;
use serde_json::Value;

use super::ProviderError;
use super::anthropic_shared::StreamOutcome;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetryAttempt {
    pub attempt: u32,
    pub max_retries: u32,
    pub delay: Duration,
    pub error: String,
}

/// Retry a fallible async operation with exponential backoff
pub async fn with_retry<F, Fut, T>(max_retries: u32, f: F) -> Result<T, ProviderError>
where
    F: Fn() -> Fut,
    Fut: Future<Output = Result<T, ProviderError>>,
{
    with_retry_if(max_retries, |error| error.is_retryable(), f).await
}

/// Retry a fallible async operation when `should_retry` accepts the error.
pub async fn with_retry_if<F, Fut, T, P>(
    max_retries: u32,
    should_retry: P,
    f: F,
) -> Result<T, ProviderError>
where
    F: Fn() -> Fut,
    Fut: Future<Output = Result<T, ProviderError>>,
    P: Fn(&ProviderError) -> bool,
{
    with_retry_if_notify(max_retries, should_retry, |_| {}, f).await
}

/// Retry a fallible async operation and notify before each backoff sleep.
pub async fn with_retry_if_notify<F, Fut, T, P, N>(
    max_retries: u32,
    should_retry: P,
    on_retry: N,
    f: F,
) -> Result<T, ProviderError>
where
    F: Fn() -> Fut,
    Fut: Future<Output = Result<T, ProviderError>>,
    P: Fn(&ProviderError) -> bool,
    N: Fn(RetryAttempt),
{
    with_retry_if_notify_budget(max_retries, should_retry, |_| max_retries, on_retry, f).await
}

/// Retry a fallible async operation and choose the retry budget from the last error.
pub async fn with_retry_if_notify_budget<F, Fut, T, P, B, N>(
    max_retries: u32,
    should_retry: P,
    retry_budget: B,
    on_retry: N,
    f: F,
) -> Result<T, ProviderError>
where
    F: Fn() -> Fut,
    Fut: Future<Output = Result<T, ProviderError>>,
    P: Fn(&ProviderError) -> bool,
    B: Fn(&ProviderError) -> u32,
    N: Fn(RetryAttempt),
{
    let mut backoff = Duration::from_secs(1);
    for attempt in 0..=max_retries {
        match f().await {
            Ok(val) => return Ok(val),
            Err(e) if should_retry(&e) && attempt < retry_budget(&e).min(max_retries) => {
                let max_retries = retry_budget(&e).min(max_retries);
                eprintln!("[retry] attempt {}/{}: {}", attempt + 1, max_retries, e);
                let delay = retry_delay(&e, backoff);
                on_retry(RetryAttempt {
                    attempt: attempt + 1,
                    max_retries,
                    delay,
                    error: e.to_string(),
                });
                tokio::time::sleep(delay).await;
                backoff = (backoff * 2).min(Duration::from_secs(30));
            }
            Err(e) => return Err(e),
        }
    }
    unreachable!()
}

pub(crate) fn retry_delay(error: &ProviderError, fallback: Duration) -> Duration {
    match error {
        ProviderError::RateLimited { retry_after_ms } => {
            Duration::from_millis(*retry_after_ms).min(Duration::from_secs(30))
        }
        _ => jitter_delay(fallback),
    }
}

fn jitter_delay(delay: Duration) -> Duration {
    let jitter = rand::thread_rng().gen_range(0.9..=1.1);
    jitter_delay_with_factor(delay, jitter)
}

fn jitter_delay_with_factor(delay: Duration, factor: f64) -> Duration {
    let millis = delay.as_millis() as f64 * factor;
    Duration::from_millis(millis.round().max(1.0) as u64)
}

pub const MAX_STREAM_RETRIES: u32 = 2;
const MAX_STREAM_BACKOFF: Duration = Duration::from_secs(15);

/// Send an HTTP request and check status, returning the response on success.
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

/// Sleep with exponential backoff before retrying an empty failed stream.
pub async fn backoff_sleep(attempt: u32, current_backoff: Duration) -> Duration {
    tracing::warn!(
        target: "aion_providers",
        attempt,
        max = MAX_STREAM_RETRIES,
        "retrying stream after mid-stream disconnect"
    );
    tokio::time::sleep(current_backoff).await;
    (current_backoff * 2).min(MAX_STREAM_BACKOFF)
}

/// Evaluate a stream outcome inside a retry loop.
pub fn evaluate_outcome(
    outcome: StreamOutcome,
    attempt: u32,
) -> Result<Option<ProviderError>, ProviderError> {
    match outcome {
        StreamOutcome::Ok => Ok(None),
        StreamOutcome::FailedPartial(e) => Ok(Some(e)),
        StreamOutcome::FailedEmpty(e) => {
            if attempt == MAX_STREAM_RETRIES {
                Ok(Some(e))
            } else {
                Err(e)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;
    use crate::ProviderError;

    #[tokio::test]
    async fn test_retry_succeeds_first_try() {
        let result = with_retry(2, || async { Ok::<_, ProviderError>(42) }).await;
        assert_eq!(result.unwrap(), 42);
    }

    #[tokio::test]
    async fn test_retry_succeeds_after_failures() {
        // Pause tokio time so sleep calls return immediately
        tokio::time::pause();

        let counter = Arc::new(AtomicU32::new(0));
        let result = with_retry(2, || {
            let counter = Arc::clone(&counter);
            async move {
                let attempt = counter.fetch_add(1, Ordering::SeqCst);
                if attempt < 2 {
                    Err(ProviderError::Connection("timeout".into()))
                } else {
                    Ok(attempt)
                }
            }
        })
        .await;

        assert!(result.is_ok());
        assert_eq!(counter.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn test_retry_exhausted() {
        tokio::time::pause();

        let result = with_retry(2, || async {
            Err::<(), _>(ProviderError::Connection("always fails".into()))
        })
        .await;

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ProviderError::Connection(_)));
    }

    #[tokio::test]
    async fn test_retry_non_retryable_error_fails_immediately() {
        let counter = Arc::new(AtomicU32::new(0));
        let result = with_retry(2, || {
            let counter = Arc::clone(&counter);
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                Err::<(), _>(ProviderError::Api {
                    status: 401,
                    message: "unauthorized".into(),
                })
            }
        })
        .await;

        // Non-retryable errors should fail immediately without retrying
        assert!(result.is_err());
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_retry_retries_transient_api_errors() {
        tokio::time::pause();

        let counter = Arc::new(AtomicU32::new(0));
        let result = with_retry(2, || {
            let counter = Arc::clone(&counter);
            async move {
                let attempt = counter.fetch_add(1, Ordering::SeqCst);
                if attempt == 0 {
                    Err(ProviderError::Api {
                        status: 503,
                        message: "service unavailable".into(),
                    })
                } else {
                    Ok(attempt)
                }
            }
        })
        .await;

        assert!(result.is_ok());
        assert_eq!(counter.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn test_retry_notifies_before_backoff() {
        tokio::time::pause();

        let attempts = Arc::new(AtomicU32::new(0));
        let notifications = Arc::new(Mutex::new(Vec::new()));
        let notifications_for_callback = Arc::clone(&notifications);

        let result = with_retry_if_notify(
            2,
            |error| error.is_retryable(),
            |retry| {
                notifications_for_callback.lock().unwrap().push(retry);
            },
            || {
                let attempts = Arc::clone(&attempts);
                async move {
                    let attempt = attempts.fetch_add(1, Ordering::SeqCst);
                    if attempt == 0 {
                        Err(ProviderError::RateLimited {
                            retry_after_ms: 5000,
                        })
                    } else {
                        Ok(attempt)
                    }
                }
            },
        )
        .await;

        assert!(result.is_ok());
        let notifications = notifications.lock().unwrap();
        assert_eq!(notifications.len(), 1);
        assert_eq!(notifications[0].attempt, 1);
        assert_eq!(notifications[0].max_retries, 2);
        assert_eq!(notifications[0].delay.as_millis(), 5000);
        assert_eq!(notifications[0].error, "Rate limited, retry after 5000ms");
    }

    #[tokio::test]
    async fn test_retry_budget_can_be_selected_by_error() {
        tokio::time::pause();

        let attempts = Arc::new(AtomicU32::new(0));
        let result = with_retry_if_notify_budget(
            3,
            |error| error.is_retryable(),
            |error| match error {
                ProviderError::RateLimited { .. } => 3,
                _ => 0,
            },
            |_| {},
            || {
                let attempts = Arc::clone(&attempts);
                async move {
                    let attempt = attempts.fetch_add(1, Ordering::SeqCst);
                    if attempt < 2 {
                        Err(ProviderError::RateLimited { retry_after_ms: 0 })
                    } else {
                        Ok(attempt)
                    }
                }
            },
        )
        .await;

        assert_eq!(result.unwrap(), 2);
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn test_retry_jitter_bounds_for_connection_errors() {
        for _ in 0..32 {
            let delay = retry_delay(
                &ProviderError::Connection("temporary reset".into()),
                Duration::from_secs(1),
            )
            .as_millis();
            assert!(
                (900..=1100).contains(&delay),
                "delay {delay}ms should stay within the Codex-style jitter window"
            );
        }
        assert_eq!(
            jitter_delay_with_factor(Duration::from_secs(1), 0.9),
            Duration::from_millis(900)
        );
        assert_eq!(
            jitter_delay_with_factor(Duration::from_secs(1), 1.1),
            Duration::from_millis(1100)
        );
    }
}
