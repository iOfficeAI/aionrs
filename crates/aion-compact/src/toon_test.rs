use super::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_uniform_array() {
        let json = r#"[
            {"id": 1, "name": "Alice", "role": "admin"},
            {"id": 2, "name": "Bob", "role": "user"}
        ]"#;
        let value: serde_json::Value = serde_json::from_str(json).unwrap();
        let result = toon_encode_array(&value);
        assert!(result.is_some());
        let encoded = result.unwrap();
        assert!(encoded.contains("[2]{id,name,role}:"), "should have header: {encoded}");
        assert!(encoded.contains("1,Alice,admin"));
        assert!(encoded.contains("2,Bob,user"));
    }

    #[test]
    fn encode_non_uniform_array_returns_none() {
        let json = r#"[{"id": 1}, {"name": "Bob"}]"#;
        let value: serde_json::Value = serde_json::from_str(json).unwrap();
        assert!(toon_encode_array(&value).is_none());
    }

    #[test]
    fn encode_nested_values_returns_none() {
        let json = r#"[{"id": 1, "meta": {"x": 1}}]"#;
        let value: serde_json::Value = serde_json::from_str(json).unwrap();
        assert!(toon_encode_array(&value).is_none());
    }

    #[test]
    fn encode_empty_array_returns_none() {
        let value = serde_json::json!([]);
        assert!(toon_encode_array(&value).is_none());
    }

    #[test]
    fn encode_single_element() {
        let json = r#"[{"id": 1, "name": "Alice"}]"#;
        let value: serde_json::Value = serde_json::from_str(json).unwrap();
        let result = toon_encode_array(&value);
        assert!(result.is_some());
        let encoded = result.unwrap();
        assert!(encoded.contains("[1]{id,name}:"));
    }

    #[test]
    fn encode_values_with_commas_quoted() {
        let json = r#"[{"name": "Alice, Jr.", "age": 30}]"#;
        let value: serde_json::Value = serde_json::from_str(json).unwrap();
        let result = toon_encode_array(&value);
        assert!(result.is_some());
        let encoded = result.unwrap();
        assert!(
            encoded.contains("\"Alice, Jr.\""),
            "comma in value should be quoted: {encoded}"
        );
    }

    #[test]
    fn toon_prompt_instructions_not_empty() {
        let instructions = toon_format_instructions();
        assert!(!instructions.is_empty());
        assert!(instructions.contains("TOON"));
    }

    #[test]
    fn try_toon_encode_text_with_json_array() {
        let input = "Exit code: 0\nSTDOUT:\n[{\"id\":1,\"name\":\"Alice\",\"role\":\"admin\"},{\"id\":2,\"name\":\"Bob\",\"role\":\"user\"}]\nSTDERR:\n";
        let result = try_toon_encode(input);
        assert!(
            result.contains("[2]{id,name,role}:"),
            "should contain TOON header: {result}"
        );
        assert!(result.contains("Exit code: 0"), "should preserve prefix");
    }
}
