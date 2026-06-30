use super::*;

#[cfg(test)]
mod tests {
    use super::*;

    // -- MemoryType::parse --------------------------------------------------

    #[test]
    fn parse_valid_types() {
        assert_eq!(MemoryType::parse("user"), Some(MemoryType::User));
        assert_eq!(MemoryType::parse("feedback"), Some(MemoryType::Feedback));
        assert_eq!(MemoryType::parse("project"), Some(MemoryType::Project));
        assert_eq!(MemoryType::parse("reference"), Some(MemoryType::Reference));
    }

    #[test]
    fn parse_invalid_returns_none() {
        assert_eq!(MemoryType::parse("invalid"), None);
        assert_eq!(MemoryType::parse(""), None);
        assert_eq!(MemoryType::parse("User"), None); // case-sensitive
        assert_eq!(MemoryType::parse("USER"), None);
        assert_eq!(MemoryType::parse("Feedback"), None);
    }

    // -- Display + FromStr roundtrip ----------------------------------------

    #[test]
    fn display_roundtrip() {
        for ty in MemoryType::ALL {
            let s = ty.to_string();
            let parsed: MemoryType = s.parse().unwrap();
            assert_eq!(parsed, ty);
        }
    }

    #[test]
    fn display_is_lowercase() {
        assert_eq!(MemoryType::User.to_string(), "user");
        assert_eq!(MemoryType::Feedback.to_string(), "feedback");
        assert_eq!(MemoryType::Project.to_string(), "project");
        assert_eq!(MemoryType::Reference.to_string(), "reference");
    }

    // -- Serde roundtrip ----------------------------------------------------

    #[test]
    fn serde_yaml_roundtrip() {
        for ty in MemoryType::ALL {
            let yaml = serde_yaml::to_string(&ty).unwrap();
            let parsed: MemoryType = serde_yaml::from_str(&yaml).unwrap();
            assert_eq!(parsed, ty);
        }
    }

    #[test]
    fn serde_yaml_serializes_lowercase() {
        let yaml = serde_yaml::to_string(&MemoryType::User).unwrap();
        assert_eq!(yaml.trim(), "user");
    }

    #[test]
    fn serde_yaml_rejects_uppercase() {
        let result: Result<MemoryType, _> = serde_yaml::from_str("User");
        assert!(result.is_err());
    }

    // -- MemoryFrontmatter --------------------------------------------------

    #[test]
    fn frontmatter_deserialize_full() {
        let yaml = "name: test\ndescription: a test\ntype: feedback\n";
        let fm: MemoryFrontmatter = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(fm.name.as_deref(), Some("test"));
        assert_eq!(fm.description.as_deref(), Some("a test"));
        assert_eq!(fm.memory_type, Some(MemoryType::Feedback));
    }

    #[test]
    fn frontmatter_deserialize_partial() {
        let yaml = "name: partial\n";
        let fm: MemoryFrontmatter = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(fm.name.as_deref(), Some("partial"));
        assert_eq!(fm.description, None);
        assert_eq!(fm.memory_type, None);
    }

    #[test]
    fn frontmatter_deserialize_empty() {
        let fm: MemoryFrontmatter = serde_yaml::from_str("{}").unwrap();
        assert_eq!(fm, MemoryFrontmatter::default());
    }

    #[test]
    fn frontmatter_serialize_roundtrip() {
        let fm = MemoryFrontmatter {
            name: Some("my memory".into()),
            description: Some("desc".into()),
            memory_type: Some(MemoryType::Project),
        };
        let yaml = serde_yaml::to_string(&fm).unwrap();
        let parsed: MemoryFrontmatter = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(parsed, fm);
    }

    // -- MemoryEntry --------------------------------------------------------

    #[test]
    fn entry_build_convenience() {
        let entry = MemoryEntry::build("name", "desc", MemoryType::User, "body");
        assert_eq!(entry.frontmatter.name.as_deref(), Some("name"));
        assert_eq!(entry.frontmatter.description.as_deref(), Some("desc"));
        assert_eq!(entry.frontmatter.memory_type, Some(MemoryType::User));
        assert_eq!(entry.content, "body");
    }

    // -- MemoryType::ALL covers all variants --------------------------------

    #[test]
    fn all_constant_is_exhaustive() {
        assert_eq!(MemoryType::ALL.len(), 4);
        // Ensure no duplicates
        let mut seen = std::collections::HashSet::new();
        for ty in MemoryType::ALL {
            assert!(seen.insert(ty), "duplicate in ALL: {ty}");
        }
    }

    // -- ParseMemoryTypeError -----------------------------------------------

    #[test]
    fn parse_error_displays_value() {
        let err = ParseMemoryTypeError("bad".into());
        let msg = err.to_string();
        assert!(msg.contains("bad"), "error should mention the bad value");
    }
}
