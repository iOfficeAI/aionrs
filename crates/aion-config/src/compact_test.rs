use super::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_values_match_spec() {
        let cfg = CompactConfig::default();
        assert_eq!(cfg.context_window, 200_000);
        assert_eq!(cfg.output_reserve, 20_000);
        assert_eq!(cfg.autocompact_buffer, 13_000);
        assert_eq!(cfg.emergency_buffer, 3_000);
        assert_eq!(cfg.max_failures, 3);
        assert_eq!(cfg.micro_keep_recent, 5);
        assert_eq!(cfg.micro_gap_seconds, 3600);
        assert!(cfg.enabled);
        assert_eq!(cfg.autocompact_threshold_pct, None);
        assert_eq!(
            cfg.compactable_tools,
            vec!["Read", "ExecCommand", "Grep", "Glob", "Write", "Edit"]
        );
    }

    #[test]
    fn toml_full_override() {
        let toml_str = r#"
context_window = 128000
output_reserve = 10000
autocompact_buffer = 8000
emergency_buffer = 2000
max_failures = 5
micro_keep_recent = 3
micro_gap_seconds = 1800
compactable_tools = ["Read", "ExecCommand"]
enabled = false
"#;
        let cfg: CompactConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.context_window, 128_000);
        assert_eq!(cfg.output_reserve, 10_000);
        assert_eq!(cfg.autocompact_buffer, 8_000);
        assert_eq!(cfg.emergency_buffer, 2_000);
        assert_eq!(cfg.max_failures, 5);
        assert_eq!(cfg.micro_keep_recent, 3);
        assert_eq!(cfg.micro_gap_seconds, 1800);
        assert_eq!(cfg.compactable_tools, vec!["Read", "ExecCommand"]);
        assert!(!cfg.enabled);
    }

    #[test]
    fn toml_partial_override_uses_defaults() {
        let toml_str = r#"
context_window = 128000
"#;
        let cfg: CompactConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.context_window, 128_000);
        // Everything else should be default
        assert_eq!(cfg.output_reserve, 20_000);
        assert_eq!(cfg.autocompact_buffer, 13_000);
        assert_eq!(cfg.emergency_buffer, 3_000);
        assert_eq!(cfg.max_failures, 3);
        assert_eq!(cfg.micro_keep_recent, 5);
        assert_eq!(cfg.micro_gap_seconds, 3600);
        assert!(cfg.enabled);
    }

    #[test]
    fn toml_empty_uses_all_defaults() {
        let cfg: CompactConfig = toml::from_str("").unwrap();
        let default = CompactConfig::default();
        assert_eq!(cfg.context_window, default.context_window);
        assert_eq!(cfg.output_reserve, default.output_reserve);
        assert_eq!(cfg.autocompact_buffer, default.autocompact_buffer);
        assert_eq!(cfg.emergency_buffer, default.emergency_buffer);
        assert_eq!(cfg.max_failures, default.max_failures);
        assert_eq!(cfg.micro_keep_recent, default.micro_keep_recent);
        assert_eq!(cfg.micro_gap_seconds, default.micro_gap_seconds);
        assert_eq!(cfg.enabled, default.enabled);
    }

    #[test]
    fn cache_diagnostics_defaults_to_false() {
        let cfg = CompactConfig::default();
        assert!(!cfg.cache_diagnostics);
    }

    #[test]
    fn toml_cache_diagnostics_override() {
        let toml_str = r#"
cache_diagnostics = true
"#;
        let cfg: CompactConfig = toml::from_str(toml_str).unwrap();
        assert!(cfg.cache_diagnostics);
    }

    #[test]
    fn default_compaction_is_safe() {
        let cfg = CompactConfig::default();
        assert_eq!(cfg.compaction, aion_compact::CompactLevel::Safe);
    }

    #[test]
    fn default_toon_is_false() {
        let cfg = CompactConfig::default();
        assert!(!cfg.toon);
    }

    #[test]
    fn toml_compaction_level_override() {
        let toml_str = r#"compaction = "full""#;
        let cfg: CompactConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.compaction, aion_compact::CompactLevel::Full);
    }

    #[test]
    fn toml_compaction_off() {
        let toml_str = r#"compaction = "off""#;
        let cfg: CompactConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.compaction, aion_compact::CompactLevel::Off);
    }

    #[test]
    fn toml_toon_enabled() {
        let toml_str = r#"toon = true"#;
        let cfg: CompactConfig = toml::from_str(toml_str).unwrap();
        assert!(cfg.toon);
    }

    #[test]
    fn json_serialization_roundtrip() {
        let cfg = CompactConfig {
            context_window: 100_000,
            output_reserve: 15_000,
            ..Default::default()
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let back: CompactConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.context_window, 100_000);
        assert_eq!(back.output_reserve, 15_000);
        assert_eq!(back.autocompact_buffer, cfg.autocompact_buffer);
    }

    #[test]
    fn toml_autocompact_threshold_pct() {
        let toml_str = r#"autocompact_threshold_pct = 50"#;
        let cfg: CompactConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.autocompact_threshold_pct, Some(50));
    }

    #[test]
    fn toml_absent_threshold_pct_is_none() {
        let toml_str = r#"context_window = 128000"#;
        let cfg: CompactConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.autocompact_threshold_pct, None);
    }
}
