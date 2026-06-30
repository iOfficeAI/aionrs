use super::*;

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
        assert!(ProviderError::RateLimited { retry_after_ms: 1000 }.is_retryable());
        assert!(ProviderError::Connection("x".into()).is_retryable());
    }
}
