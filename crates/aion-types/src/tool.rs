use serde_json::Value;

/// Schema for a tool parameter, in JSON Schema format
pub type JsonSchema = Value;

/// Definition of a tool for the API
#[derive(Debug, Clone)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    pub input_schema: JsonSchema,
}

/// Result from executing a tool
#[derive(Debug, Clone)]
pub struct ToolResult {
    pub content: String,
    pub is_error: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // --- ToolDef construction and field validation ---

    #[test]
    fn test_tool_def_construction_fields() {
        // arrange
        let schema = json!({
            "type": "object",
            "properties": {
                "cmd": { "type": "string" }
            },
            "required": ["cmd"]
        });
        // act
        let tool = ToolDef {
            name: "bash".to_string(),
            description: "Run a shell command".to_string(),
            input_schema: schema.clone(),
        };
        // assert
        assert_eq!(tool.name, "bash");
        assert_eq!(tool.description, "Run a shell command");
        assert_eq!(tool.input_schema, schema);
    }

    #[test]
    fn test_tool_def_empty_schema_is_valid() {
        // arrange + act
        let tool = ToolDef {
            name: "noop".to_string(),
            description: "Does nothing".to_string(),
            input_schema: json!({}),
        };
        // assert
        assert_eq!(tool.input_schema, json!({}));
    }

    // --- ToolResult success scenario ---

    #[test]
    fn test_tool_result_success_is_error_false() {
        // arrange + act
        let result = ToolResult {
            content: "command output".to_string(),
            is_error: false,
        };
        // assert
        assert_eq!(result.content, "command output");
        assert!(!result.is_error);
    }

    // --- ToolResult error scenario ---

    #[test]
    fn test_tool_result_error_is_error_true() {
        // arrange + act
        let result = ToolResult {
            content: "permission denied".to_string(),
            is_error: true,
        };
        // assert
        assert_eq!(result.content, "permission denied");
        assert!(result.is_error);
    }

    #[test]
    fn test_tool_result_error_empty_content() {
        // arrange + act – errors may carry an empty content string
        let result = ToolResult {
            content: String::new(),
            is_error: true,
        };
        // assert
        assert!(result.content.is_empty());
        assert!(result.is_error);
    }
}
