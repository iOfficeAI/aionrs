use super::*;

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> CompactConfig {
        CompactConfig {
            max_failures: 3,
            ..Default::default()
        }
    }

    #[test]
    fn new_state_not_circuit_broken() {
        let state = CompactState::new();
        assert_eq!(state.consecutive_failures, 0);
        assert_eq!(state.last_input_tokens, 0);
        assert!(!state.is_circuit_broken(&test_config()));
    }

    #[test]
    fn circuit_breaker_trips_at_max_failures() {
        let config = test_config();
        let mut state = CompactState::new();

        state.record_failure();
        assert!(!state.is_circuit_broken(&config));
        state.record_failure();
        assert!(!state.is_circuit_broken(&config));
        state.record_failure();
        assert!(state.is_circuit_broken(&config));
    }

    #[test]
    fn success_resets_failure_counter() {
        let config = test_config();
        let mut state = CompactState::new();

        state.record_failure();
        state.record_failure();
        assert_eq!(state.consecutive_failures, 2);

        state.record_success();
        assert_eq!(state.consecutive_failures, 0);
        assert!(!state.is_circuit_broken(&config));
    }

    #[test]
    fn circuit_breaker_with_max_failures_one() {
        let config = CompactConfig {
            max_failures: 1,
            ..Default::default()
        };
        let mut state = CompactState::new();

        assert!(!state.is_circuit_broken(&config));
        state.record_failure();
        assert!(state.is_circuit_broken(&config));
    }

    #[test]
    fn default_impl_matches_new() {
        let a = CompactState::new();
        let b = CompactState::default();
        assert_eq!(a.consecutive_failures, b.consecutive_failures);
        assert_eq!(a.last_input_tokens, b.last_input_tokens);
    }
}
