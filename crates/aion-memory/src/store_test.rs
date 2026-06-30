use super::*;

// ===========================================================================
// Unit tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::MemoryError;
    use crate::types::MemoryType;

    // -- parse_frontmatter ---------------------------------------------------

    #[test]
    fn parse_full_frontmatter() {
        let raw = "---\nname: test\ndescription: a test\ntype: feedback\n---\nBody content";
        let (fm, body) = parse_frontmatter(raw, None);
        assert_eq!(fm.name.as_deref(), Some("test"));
        assert_eq!(fm.description.as_deref(), Some("a test"));
        assert_eq!(fm.memory_type, Some(MemoryType::Feedback));
        assert_eq!(body, "Body content");
    }

    #[test]
    fn parse_no_frontmatter() {
        let raw = "Just plain text\nNo frontmatter here";
        let (fm, body) = parse_frontmatter(raw, None);
        assert_eq!(fm, MemoryFrontmatter::default());
        assert_eq!(body, raw);
    }

    #[test]
    fn parse_empty_content() {
        let (fm, body) = parse_frontmatter("", None);
        assert_eq!(fm, MemoryFrontmatter::default());
        assert_eq!(body, "");
    }

    #[test]
    fn parse_only_opening_delimiter() {
        let raw = "---\nname: orphan\nno closing delimiter";
        let (fm, body) = parse_frontmatter(raw, None);
        assert_eq!(fm, MemoryFrontmatter::default());
        assert_eq!(body, raw);
    }

    #[test]
    fn parse_partial_frontmatter_fields() {
        let raw = "---\nname: partial\n---\nBody";
        let (fm, body) = parse_frontmatter(raw, None);
        assert_eq!(fm.name.as_deref(), Some("partial"));
        assert_eq!(fm.description, None);
        assert_eq!(fm.memory_type, None);
        assert_eq!(body, "Body");
    }

    #[test]
    fn parse_frontmatter_with_leading_whitespace() {
        let raw = "  \n---\nname: spaced\n---\nContent";
        let (fm, body) = parse_frontmatter(raw, None);
        assert_eq!(fm.name.as_deref(), Some("spaced"));
        assert_eq!(body, "Content");
    }

    #[test]
    fn parse_invalid_yaml_degrades_gracefully() {
        // YAML with invalid structure — should return default frontmatter
        let raw = "---\n: :\n  :\n---\nBody after bad yaml";
        let (fm, body) = parse_frontmatter(raw, None);
        assert_eq!(fm, MemoryFrontmatter::default());
        assert_eq!(body, "Body after bad yaml");
    }

    #[test]
    fn parse_frontmatter_body_newline_handling() {
        let raw = "---\nname: test\n---\n\nParagraph one\n\nParagraph two";
        let (fm, body) = parse_frontmatter(raw, None);
        assert_eq!(fm.name.as_deref(), Some("test"));
        // Body should start at first content line after delimiter
        assert_eq!(body, "Paragraph one\n\nParagraph two");
    }

    // -- serialize_entry -----------------------------------------------------

    #[test]
    fn serialize_and_parse_roundtrip() {
        let entry = MemoryEntry::build("role", "user role info", MemoryType::User, "I am a dev");
        let serialized = serialize_entry(&entry);
        let (fm, body) = parse_frontmatter(&serialized, None);
        assert_eq!(fm.name.as_deref(), Some("role"));
        assert_eq!(fm.description.as_deref(), Some("user role info"));
        assert_eq!(fm.memory_type, Some(MemoryType::User));
        assert_eq!(body, "I am a dev");
    }

    // -- generate_filename ---------------------------------------------------

    #[test]
    fn filename_with_type_and_name() {
        let fm = MemoryFrontmatter {
            name: Some("My Role".into()),
            description: None,
            memory_type: Some(MemoryType::User),
        };
        let name = generate_filename(&fm);
        assert_eq!(name, "user_my_role.md");
    }

    #[test]
    fn filename_without_type() {
        let fm = MemoryFrontmatter {
            name: Some("notes".into()),
            description: None,
            memory_type: None,
        };
        let name = generate_filename(&fm);
        assert_eq!(name, "memory_notes.md");
    }

    #[test]
    fn filename_without_name() {
        let fm = MemoryFrontmatter {
            name: None,
            description: None,
            memory_type: Some(MemoryType::Feedback),
        };
        let name = generate_filename(&fm);
        assert!(name.starts_with("feedback_"));
        assert!(name.ends_with(".md"));
    }

    #[test]
    fn filename_special_chars_sanitized() {
        let fm = MemoryFrontmatter {
            name: Some("Hello World! / Test: 123".into()),
            description: None,
            memory_type: Some(MemoryType::Project),
        };
        let name = generate_filename(&fm);
        assert_eq!(name, "project_hello_world_test_123.md");
        assert!(!name.contains(' '));
        assert!(!name.contains('/'));
        assert!(!name.contains('!'));
    }

    // -- sanitize_filename ---------------------------------------------------

    #[test]
    fn sanitize_basic() {
        assert_eq!(sanitize_filename("Hello World"), "hello_world");
    }

    #[test]
    fn sanitize_collapses_underscores() {
        assert_eq!(sanitize_filename("a---b___c"), "a_b_c");
    }

    #[test]
    fn sanitize_trims_underscores() {
        assert_eq!(sanitize_filename("__test__"), "test");
    }

    #[test]
    fn sanitize_preserves_alphanumeric() {
        assert_eq!(sanitize_filename("abc123"), "abc123");
    }

    #[test]
    fn sanitize_pure_non_ascii_returns_empty() {
        assert_eq!(sanitize_filename("我的角色"), "");
        assert_eq!(sanitize_filename("日本語"), "");
    }

    #[test]
    fn filename_pure_non_ascii_name_falls_back_to_hash() {
        let fm1 = MemoryFrontmatter {
            name: Some("我的角色".into()),
            description: None,
            memory_type: Some(MemoryType::User),
        };
        let fm2 = MemoryFrontmatter {
            name: Some("项目状态".into()),
            description: None,
            memory_type: Some(MemoryType::User),
        };
        let name1 = generate_filename(&fm1);
        let name2 = generate_filename(&fm2);
        // Both should get unique hash-based names, not collide
        assert!(name1.starts_with("user_"));
        assert!(name1.ends_with(".md"));
        assert_ne!(name1, "user_.md", "should not produce empty name part");
        // With time-based hash, names should differ (race possible but
        // extremely unlikely given nanos resolution)
        assert_ne!(name1, name2, "pure non-ASCII names should not collide");
    }

    // -- is_scannable_md -----------------------------------------------------

    #[test]
    fn scannable_normal_md() {
        assert!(is_scannable_md(Path::new("/dir/user_role.md")));
    }

    #[test]
    fn scannable_rejects_memory_md() {
        assert!(!is_scannable_md(Path::new("/dir/MEMORY.md")));
    }

    #[test]
    fn scannable_rejects_non_md() {
        assert!(!is_scannable_md(Path::new("/dir/notes.txt")));
        assert!(!is_scannable_md(Path::new("/dir/data.json")));
    }

    // -- format_memory_manifest ----------------------------------------------

    #[test]
    fn manifest_with_full_headers() {
        let headers = vec![MemoryHeader {
            filename: "user_role.md".into(),
            file_path: PathBuf::from("/mem/user_role.md"),
            mtime: Utc.with_ymd_and_hms(2026, 4, 10, 12, 0, 0).unwrap(),
            description: Some("User role info".into()),
            memory_type: Some(MemoryType::User),
        }];
        let manifest = format_memory_manifest(&headers);
        assert_eq!(manifest, "- [user] user_role.md (2026-04-10T12:00:00): User role info");
    }

    #[test]
    fn manifest_without_type_and_description() {
        let headers = vec![MemoryHeader {
            filename: "notes.md".into(),
            file_path: PathBuf::from("/mem/notes.md"),
            mtime: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
            description: None,
            memory_type: None,
        }];
        let manifest = format_memory_manifest(&headers);
        assert_eq!(manifest, "- notes.md (2026-01-01T00:00:00)");
    }

    #[test]
    fn manifest_empty() {
        assert_eq!(format_memory_manifest(&[]), "");
    }

    // -- file operations (using tempdir) -------------------------------------

    #[test]
    fn write_then_read_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let entry = MemoryEntry::build("role", "my role", MemoryType::User, "I am a developer");

        let path = write_memory(tmp.path(), &entry).unwrap();
        assert!(path.exists());
        assert_eq!(path.file_name().unwrap().to_str().unwrap(), "user_role.md");

        let read_back = read_memory(&path).unwrap();
        assert_eq!(read_back.frontmatter.name, entry.frontmatter.name);
        assert_eq!(read_back.frontmatter.description, entry.frontmatter.description);
        assert_eq!(read_back.frontmatter.memory_type, entry.frontmatter.memory_type);
        assert_eq!(read_back.content, entry.content);
    }

    #[test]
    fn delete_existing_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("test.md");
        fs::write(&path, "content").unwrap();
        assert!(path.exists());

        delete_memory(&path).unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn delete_nonexistent_file_errors() {
        let err = delete_memory(Path::new("/nonexistent/file.md")).unwrap_err();
        assert!(matches!(err, MemoryError::Io(_)));
    }

    #[test]
    fn scan_excludes_memory_md_and_non_md() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();

        // Create files
        fs::write(dir.join("user_role.md"), "---\ntype: user\n---\nBody").unwrap();
        fs::write(dir.join("MEMORY.md"), "# Index").unwrap();
        fs::write(dir.join("notes.txt"), "not markdown").unwrap();

        let headers = scan_memory_files(dir).unwrap();
        assert_eq!(headers.len(), 1);
        assert_eq!(headers[0].filename, "user_role.md");
    }

    #[test]
    fn scan_nonexistent_dir_returns_empty() {
        let headers = scan_memory_files(Path::new("/nonexistent/dir")).unwrap();
        assert!(headers.is_empty());
    }

    #[test]
    fn scan_empty_dir_returns_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let headers = scan_memory_files(tmp.path()).unwrap();
        assert!(headers.is_empty());
    }
}
