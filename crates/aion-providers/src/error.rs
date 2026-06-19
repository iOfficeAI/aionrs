#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("API error {status}: {message}")]
    Api { status: u16, message: String },
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
