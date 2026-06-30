use super::*;

#[cfg(test)]
mod tests {
    use super::*;

    fn default_config() -> CompactConfig {
        CompactConfig::default()
    }

    // ── is_at_emergency_limit ──────────────────────────────────────────

    #[test]
    fn below_limit_returns_false() {
        // limit = 200k - 3k = 197k; 190k < 197k
        let config = default_config();
        assert!(!is_at_emergency_limit(190_000, &config));
    }

    #[test]
    fn above_limit_returns_true() {
        // 198k >= 197k
        let config = default_config();
        assert!(is_at_emergency_limit(198_000, &config));
    }

    #[test]
    fn at_exact_limit_returns_true() {
        // 197k >= 197k
        let config = default_config();
        assert!(is_at_emergency_limit(197_000, &config));
    }

    #[test]
    fn small_context_window() {
        let config = CompactConfig {
            context_window: 8_000,
            emergency_buffer: 3_000,
            ..default_config()
        };
        // limit = 8k - 3k = 5k; 6k >= 5k
        assert!(is_at_emergency_limit(6_000, &config));
    }

    #[test]
    fn zero_tokens_below_limit() {
        let config = default_config();
        assert!(!is_at_emergency_limit(0, &config));
    }

    #[test]
    fn custom_emergency_buffer() {
        let config = CompactConfig {
            context_window: 100_000,
            emergency_buffer: 10_000,
            ..default_config()
        };
        // limit = 100k - 10k = 90k
        assert!(!is_at_emergency_limit(89_999, &config));
        assert!(is_at_emergency_limit(90_000, &config));
        assert!(is_at_emergency_limit(95_000, &config));
    }

    #[test]
    fn works_regardless_of_enabled_flag() {
        let config = CompactConfig {
            enabled: false,
            ..default_config()
        };
        // Emergency check ignores the enabled flag
        assert!(is_at_emergency_limit(198_000, &config));
    }

    #[test]
    fn emergency_buffer_larger_than_context_window_saturates() {
        let config = CompactConfig {
            context_window: 1_000,
            emergency_buffer: 5_000,
            ..default_config()
        };
        // saturating_sub: limit = 0; any positive token count triggers
        assert!(is_at_emergency_limit(1, &config));
        // 0 tokens = 0 >= 0 → true (degenerate but safe)
        assert!(is_at_emergency_limit(0, &config));
    }

    // ── EMERGENCY_USER_MESSAGE ─────────────────────────────────────────

    #[test]
    fn user_message_mentions_compact() {
        assert!(EMERGENCY_USER_MESSAGE.contains("/compact"));
    }

    #[test]
    fn user_message_mentions_new_conversation() {
        assert!(EMERGENCY_USER_MESSAGE.contains("new conversation"));
    }
}
