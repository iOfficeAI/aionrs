use super::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_correct() {
        let config = FileCacheConfig::default();
        assert_eq!(config.max_entries, 100);
        assert_eq!(config.max_size_bytes, 25 * 1024 * 1024);
        assert!(config.enabled);
    }

    #[test]
    fn deserialize_from_toml_full() {
        let toml_str = r#"
max_entries = 50
max_size_bytes = 10485760
enabled = false
"#;
        let config: FileCacheConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.max_entries, 50);
        assert_eq!(config.max_size_bytes, 10_485_760);
        assert!(!config.enabled);
    }

    #[test]
    fn deserialize_from_toml_partial_uses_defaults() {
        let toml_str = r#"
max_entries = 200
"#;
        let config: FileCacheConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.max_entries, 200);
        assert_eq!(config.max_size_bytes, 25 * 1024 * 1024);
        assert!(config.enabled);
    }

    #[test]
    fn deserialize_from_empty_toml() {
        let config: FileCacheConfig = toml::from_str("").unwrap();
        assert_eq!(config.max_entries, 100);
        assert_eq!(config.max_size_bytes, 25 * 1024 * 1024);
        assert!(config.enabled);
    }
}
