use super::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_all_none() {
        let cfg = LoggingConfig::default();
        assert!(cfg.enabled.is_none());
        assert!(cfg.level.is_none());
        assert!(cfg.dir.is_none());
    }

    #[test]
    fn toml_with_all_fields() {
        let toml_str = r#"
enabled = true
level = "debug"
dir = "/tmp/aionrs-logs"
"#;
        let cfg: LoggingConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.enabled, Some(true));
        assert_eq!(cfg.level.as_deref(), Some("debug"));
        assert_eq!(cfg.dir.as_deref(), Some("/tmp/aionrs-logs"));
    }

    #[test]
    fn toml_empty_uses_defaults() {
        let cfg: LoggingConfig = toml::from_str("").unwrap();
        assert!(cfg.enabled.is_none());
        assert!(cfg.level.is_none());
        assert!(cfg.dir.is_none());
    }

    #[test]
    fn merge_project_overrides_global() {
        let global = LoggingConfig {
            enabled: Some(false),
            level: Some("warn".into()),
            dir: Some("/global/logs".into()),
        };
        let project = LoggingConfig {
            enabled: Some(true),
            level: Some("debug".into()),
            dir: None,
        };
        let merged = LoggingConfig::merge(global, project);
        assert_eq!(merged.enabled, Some(true));
        assert_eq!(merged.level.as_deref(), Some("debug"));
        assert_eq!(merged.dir.as_deref(), Some("/global/logs"));
    }

    #[test]
    fn merge_falls_back_to_global() {
        let global = LoggingConfig {
            level: Some("info".into()),
            ..Default::default()
        };
        let project = LoggingConfig::default();
        let merged = LoggingConfig::merge(global, project);
        assert_eq!(merged.level.as_deref(), Some("info"));
    }

    #[test]
    fn merge_two_empty_configs() {
        let merged = LoggingConfig::merge(LoggingConfig::default(), LoggingConfig::default());
        assert!(merged.enabled.is_none());
        assert!(merged.level.is_none());
        assert!(merged.dir.is_none());
    }

    #[test]
    fn resolve_dir_set_implies_enabled() {
        let cfg = LoggingConfig {
            dir: Some("/tmp/logs".into()),
            ..Default::default()
        };
        let resolved = cfg.resolve(None, None);
        assert!(resolved.enabled);
        assert_eq!(resolved.dir, PathBuf::from("/tmp/logs"));
        assert_eq!(resolved.level, "info");
    }

    #[test]
    fn resolve_nothing_set_means_disabled() {
        let cfg = LoggingConfig::default();
        let resolved = cfg.resolve(None, None);
        assert!(!resolved.enabled);
    }

    #[test]
    fn resolve_cli_overrides_config() {
        let cfg = LoggingConfig {
            level: Some("warn".into()),
            dir: Some("/config/logs".into()),
            ..Default::default()
        };
        let resolved = cfg.resolve(Some("/cli/logs"), Some("debug"));
        assert_eq!(resolved.dir, PathBuf::from("/cli/logs"));
        assert_eq!(resolved.level, "debug");
        assert!(resolved.enabled);
    }

    #[test]
    fn resolve_level_defaults_to_info() {
        let cfg = LoggingConfig {
            enabled: Some(true),
            ..Default::default()
        };
        let resolved = cfg.resolve(None, None);
        assert_eq!(resolved.level, "info");
    }

    #[test]
    fn default_log_dir_returns_nonempty_path() {
        let dir = default_log_dir();
        assert!(!dir.as_os_str().is_empty());
    }

    #[test]
    fn default_log_dir_contains_aionrs() {
        let dir = default_log_dir();
        let s = dir.to_string_lossy();
        assert!(s.contains("aionrs"), "expected 'aionrs' in path: {s}");
    }
}
