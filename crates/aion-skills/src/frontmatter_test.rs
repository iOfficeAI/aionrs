use super::*;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{LoadedFrom, SkillSource};

    // --- extract_frontmatter_bounds ---

    #[test]
    fn test_extract_basic_frontmatter() {
        let input = "---\nname: foo\n---\nbody text";
        let (yaml, body) = extract_frontmatter_bounds(input).unwrap();
        assert_eq!(yaml, "name: foo");
        assert_eq!(body, "body text");
    }

    #[test]
    fn test_extract_empty_frontmatter() {
        let input = "---\n---\nbody";
        let (yaml, body) = extract_frontmatter_bounds(input).unwrap();
        assert_eq!(yaml, "");
        assert_eq!(body, "body");
    }

    #[test]
    fn test_extract_no_frontmatter() {
        let input = "# Just a heading\n\nSome content";
        assert!(extract_frontmatter_bounds(input).is_none());
    }

    #[test]
    fn test_extract_empty_body() {
        let input = "---\nname: bar\n---";
        let (yaml, body) = extract_frontmatter_bounds(input).unwrap();
        assert_eq!(yaml, "name: bar");
        assert_eq!(body, "");
    }

    #[test]
    fn test_extract_empty_input() {
        assert!(extract_frontmatter_bounds("").is_none());
    }

    // --- parse_frontmatter ---

    #[test]
    fn test_parse_frontmatter_full() {
        let input = r#"---
name: my-skill
description: Does something useful
allowed-tools: Read, Write
user-invocable: true
---
# Skill body

Do the thing.
"#;
        let parsed = parse_frontmatter(input);
        assert_eq!(parsed.frontmatter.name.as_deref(), Some("my-skill"));
        assert_eq!(parsed.frontmatter.description.as_deref(), Some("Does something useful"));
        assert!(parsed.content.contains("Skill body"));
    }

    #[test]
    fn test_parse_frontmatter_empty() {
        let input = "---\n---\nbody";
        let parsed = parse_frontmatter(input);
        assert!(parsed.frontmatter.name.is_none());
        assert_eq!(parsed.content, "body");
    }

    #[test]
    fn test_parse_frontmatter_none() {
        let input = "# No frontmatter here\n\nJust content.";
        let parsed = parse_frontmatter(input);
        assert!(parsed.frontmatter.name.is_none());
        assert_eq!(parsed.content, input);
    }

    #[test]
    fn test_parse_frontmatter_malformed_yaml() {
        // Malformed YAML that can't be fixed — should return empty FrontmatterData
        let input = "---\n: {broken yaml\n---\ncontent";
        let parsed = parse_frontmatter(input);
        // Should not panic; content preserved
        assert_eq!(parsed.content, "content");
    }

    #[test]
    fn test_parse_frontmatter_special_chars_in_value() {
        // Description contains { } which would fail unquoted YAML
        let input = "---\ndescription: Use {arg} to specify the value\n---\nbody";
        let parsed = parse_frontmatter(input);
        // Second-pass auto-quoting should rescue this
        assert_eq!(
            parsed.frontmatter.description.as_deref(),
            Some("Use {arg} to specify the value")
        );
    }

    // --- expand_braces ---

    #[test]
    fn test_expand_braces_single_group() {
        let mut result = expand_braces("*.{ts,tsx}");
        result.sort();
        assert_eq!(result, vec!["*.ts", "*.tsx"]);
    }

    #[test]
    fn test_expand_braces_two_groups() {
        let mut result = expand_braces("{a,b}/{c,d}");
        result.sort();
        assert_eq!(result, vec!["a/c", "a/d", "b/c", "b/d"]);
    }

    #[test]
    fn test_expand_braces_no_braces() {
        let result = expand_braces("src/**/*.rs");
        assert_eq!(result, vec!["src/**/*.rs"]);
    }

    #[test]
    fn test_expand_braces_single_option() {
        let result = expand_braces("{only}");
        assert_eq!(result, vec!["only"]);
    }

    // --- parse_bool ---

    #[test]
    fn test_parse_bool_true_bool() {
        assert!(parse_bool(&Some(BoolOrString::Bool(true)), false));
    }

    #[test]
    fn test_parse_bool_false_bool() {
        assert!(!parse_bool(&Some(BoolOrString::Bool(false)), true));
    }

    #[test]
    fn test_parse_bool_string_true() {
        assert!(parse_bool(&Some(BoolOrString::Str("true".into())), false));
        assert!(parse_bool(&Some(BoolOrString::Str("TRUE".into())), false));
    }

    #[test]
    fn test_parse_bool_string_false() {
        assert!(!parse_bool(&Some(BoolOrString::Str("false".into())), true));
    }

    #[test]
    fn test_parse_bool_none_returns_default() {
        assert!(parse_bool(&None, true));
        assert!(!parse_bool(&None, false));
    }

    // --- parse_effort ---

    #[test]
    fn test_parse_effort_strings() {
        assert_eq!(
            parse_effort(&Some(StringOrNumber::Str("low".into()))),
            Some(EffortLevel::Low)
        );
        assert_eq!(
            parse_effort(&Some(StringOrNumber::Str("medium".into()))),
            Some(EffortLevel::Medium)
        );
        assert_eq!(
            parse_effort(&Some(StringOrNumber::Str("high".into()))),
            Some(EffortLevel::High)
        );
        assert_eq!(
            parse_effort(&Some(StringOrNumber::Str("max".into()))),
            Some(EffortLevel::Max)
        );
    }

    #[test]
    fn test_parse_effort_numbers() {
        assert_eq!(parse_effort(&Some(StringOrNumber::Num(0))), Some(EffortLevel::Low));
        assert_eq!(parse_effort(&Some(StringOrNumber::Num(1))), Some(EffortLevel::Medium));
        assert_eq!(parse_effort(&Some(StringOrNumber::Num(2))), Some(EffortLevel::High));
        assert_eq!(parse_effort(&Some(StringOrNumber::Num(99))), Some(EffortLevel::Max));
    }

    #[test]
    fn test_parse_effort_none() {
        assert_eq!(parse_effort(&None), None);
    }

    // --- parse_string_or_vec ---

    #[test]
    fn test_parse_string_or_vec_single_comma() {
        let v = parse_string_or_vec(&Some(StringOrVec::Single("Read, Write, Bash".into())));
        assert_eq!(v, vec!["Read", "Write", "Bash"]);
    }

    #[test]
    fn test_parse_string_or_vec_multiple() {
        let v = parse_string_or_vec(&Some(StringOrVec::Multiple(vec!["Read".into(), "Write".into()])));
        assert_eq!(v, vec!["Read", "Write"]);
    }

    #[test]
    fn test_parse_string_or_vec_none() {
        let v = parse_string_or_vec(&None);
        assert!(v.is_empty());
    }

    // --- quote_problematic_values ---

    #[test]
    fn test_quote_curly_braces() {
        let yaml = "description: Use {arg} here";
        let fixed = quote_problematic_values(yaml);
        assert!(fixed.contains("\"Use {arg} here\""));
    }

    #[test]
    fn test_quote_already_quoted_untouched() {
        let yaml = "description: \"already quoted\"";
        let fixed = quote_problematic_values(yaml);
        // Should not double-quote
        assert_eq!(fixed.trim(), yaml);
    }

    #[test]
    fn test_quote_nested_lines_untouched() {
        let yaml = "hooks:\n  - match: foo\n    value: {bar}";
        let fixed = quote_problematic_values(yaml);
        // Indented lines must not be modified
        assert!(fixed.contains("  - match: foo"));
        assert!(fixed.contains("    value: {bar}"));
    }

    // --- parse_skill_fields ---

    #[test]
    fn test_parse_skill_fields_defaults() {
        let fm = FrontmatterData::default();
        let meta = parse_skill_fields(
            &fm,
            "# My skill\n\nDoes things.",
            "my-skill",
            SkillSource::User,
            LoadedFrom::Skills,
            None,
        );
        assert_eq!(meta.name, "my-skill");
        assert!(meta.user_invocable); // default true
        assert!(!meta.disable_model_invocation); // default false
        assert_eq!(meta.execution_context, ExecutionContext::Inline);
        assert!(meta.model.is_none());
        // description falls back to first non-empty content line
        assert_eq!(meta.description, "Does things.");
    }

    #[test]
    fn test_parse_skill_fields_model_inherit() {
        let fm = FrontmatterData {
            model: Some("inherit".into()),
            ..Default::default()
        };
        let meta = parse_skill_fields(&fm, "", "x", SkillSource::Project, LoadedFrom::Skills, None);
        assert!(meta.model.is_none());
    }

    #[test]
    fn test_parse_skill_fields_fork_context() {
        let fm = FrontmatterData {
            context: Some("fork".into()),
            ..Default::default()
        };
        let meta = parse_skill_fields(&fm, "", "x", SkillSource::User, LoadedFrom::Skills, None);
        assert_eq!(meta.execution_context, ExecutionContext::Fork);
    }

    #[test]
    fn test_parse_skill_fields_paths_brace_expansion() {
        let fm = FrontmatterData {
            paths: Some(StringOrVec::Single("src/*.{ts,tsx}".into())),
            ..Default::default()
        };
        let meta = parse_skill_fields(&fm, "", "x", SkillSource::User, LoadedFrom::Skills, None);
        let mut paths = meta.paths.clone();
        paths.sort();
        assert_eq!(paths, vec!["src/*.ts", "src/*.tsx"]);
    }

    #[test]
    fn test_parse_skill_fields_content_length() {
        let fm = FrontmatterData::default();
        let body = "Hello world";
        let meta = parse_skill_fields(&fm, body, "x", SkillSource::User, LoadedFrom::Skills, None);
        assert_eq!(meta.content_length, body.len());
    }
}

// Supplemental tests live in a separate file to keep this file under 800 lines.
#[cfg(test)]
#[path = "frontmatter_supplemental_test.rs"]
mod frontmatter_supplemental_test;
