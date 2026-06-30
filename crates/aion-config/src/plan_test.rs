use super::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_values_match_spec() {
        let cfg = PlanConfig::default();
        assert!(cfg.enabled);
        assert_eq!(cfg.plan_directory, ".aionrs/plans");
    }

    #[test]
    fn toml_full_override() {
        let toml_str = r#"
enabled = false
plan_directory = "/custom/plans"
"#;
        let cfg: PlanConfig = toml::from_str(toml_str).unwrap();
        assert!(!cfg.enabled);
        assert_eq!(cfg.plan_directory, "/custom/plans");
    }

    #[test]
    fn toml_partial_override_uses_defaults() {
        let toml_str = r#"
enabled = false
"#;
        let cfg: PlanConfig = toml::from_str(toml_str).unwrap();
        assert!(!cfg.enabled);
        assert_eq!(cfg.plan_directory, ".aionrs/plans");
    }

    #[test]
    fn toml_empty_uses_all_defaults() {
        let cfg: PlanConfig = toml::from_str("").unwrap();
        assert!(cfg.enabled);
        assert_eq!(cfg.plan_directory, ".aionrs/plans");
    }

    #[test]
    fn json_serialization_roundtrip() {
        let cfg = PlanConfig {
            enabled: false,
            plan_directory: "/tmp/plans".to_string(),
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let back: PlanConfig = serde_json::from_str(&json).unwrap();
        assert!(!back.enabled);
        assert_eq!(back.plan_directory, "/tmp/plans");
    }
}
