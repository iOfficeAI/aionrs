use super::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_4space_to_2space() {
        let input = r#"{
    "name": "Alice Wonderland",
    "email": "alice@example.com",
    "age": 30,
    "address": "123 Main Street, Anytown, USA 12345",
    "phone": "+1-555-0123"
}"#;
        let result = compact_json(input);
        assert!(result.contains("  \"name\""), "should use 2-space indent: {result}");
        assert!(!result.contains("    \"name\""), "should not have 4-space indent");
    }

    #[test]
    fn compact_short_object_inline() {
        let input = r#"{
    "user": {
        "id": 1,
        "name": "Alice"
    }
}"#;
        let result = compact_json(input);
        assert!(
            result.contains(r#"{"id":1,"name":"Alice"}"#) || result.contains(r#"{"id": 1, "name": "Alice"}"#),
            "short nested object should be inlined: {result}"
        );
    }

    #[test]
    fn compact_non_json_unchanged() {
        let input = "This is not JSON\njust plain text";
        assert_eq!(compact_json(input), input);
    }

    #[test]
    fn compact_already_minified() {
        let input = r#"{"id":1,"name":"Alice"}"#;
        let result = compact_json(input);
        assert_eq!(result.len(), input.len(), "already compact JSON should not grow");
    }

    #[test]
    fn compact_preserves_array_structure() {
        let input = r#"[
    {
        "id": 1,
        "name": "Alice"
    },
    {
        "id": 2,
        "name": "Bob"
    }
]"#;
        let result = compact_json(input);
        assert!(result.len() < input.len(), "should be shorter than input");
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed[0]["name"], "Alice");
        assert_eq!(parsed[1]["name"], "Bob");
    }

    #[test]
    fn compact_mixed_text_with_json_block() {
        let input = "Exit code: 0\nSTDOUT:\n{\n    \"status\": \"ok\"\n}\nSTDERR:\n";
        let result = compact_json(input);
        assert!(result.contains("\"status\""));
    }

    #[test]
    fn compact_empty_input() {
        assert_eq!(compact_json(""), "");
    }
}
