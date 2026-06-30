use super::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn io_error_display() {
        let inner = std::io::Error::new(std::io::ErrorKind::NotFound, "gone");
        let err = MemoryError::Io(inner);
        let msg = err.to_string();
        assert!(msg.contains("I/O"), "should mention I/O: {msg}");
        assert!(msg.contains("gone"), "should contain inner message: {msg}");
    }

    #[test]
    fn io_error_from_conversion() {
        let inner = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied");
        let err: MemoryError = inner.into();
        assert!(matches!(err, MemoryError::Io(_)));
    }

    #[test]
    fn path_validation_display() {
        let err = MemoryError::PathValidation("relative path".into());
        let msg = err.to_string();
        assert!(msg.contains("relative path"), "should contain reason: {msg}");
        assert!(msg.contains("validation"), "should mention validation: {msg}");
    }

    #[test]
    fn frontmatter_parse_display() {
        // Trigger a real serde_yaml error
        let yaml_err = serde_yaml::from_str::<serde_yaml::Value>(":\n  :\n---").unwrap_err();
        let err = MemoryError::FrontmatterParse {
            path: PathBuf::from("/tmp/test.md"),
            source: yaml_err,
        };
        let msg = err.to_string();
        assert!(msg.contains("/tmp/test.md"), "should contain path: {msg}");
        assert!(msg.contains("frontmatter"), "should mention frontmatter: {msg}");
    }
}
