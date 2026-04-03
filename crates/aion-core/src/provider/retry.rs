use std::future::Future;
use std::time::Duration;

use super::ProviderError;

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
                eprintln!("[retry] attempt {}/{}: {}", attempt + 1, max_retries, e);
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(Duration::from_secs(30));
            }
            Err(e) => return Err(e),
        }
    }
    unreachable!()
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    use super::*;
    use crate::provider::ProviderError;

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
}
