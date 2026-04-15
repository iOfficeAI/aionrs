use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};

use aion_protocol::events::ToolCategory;
use aion_types::tool::{JsonSchema, ToolResult};

use crate::Tool;
use crate::registry::ToolRegistry;

/// Built-in tool that searches for deferred tools and loads their full schema.
/// Core tool (never deferred itself) — always available to the LLM.
pub struct ToolSearchTool {
    registry: Arc<ToolRegistry>,
}

impl ToolSearchTool {
    pub fn new(registry: Arc<ToolRegistry>) -> Self {
        Self { registry }
    }
}

#[async_trait]
impl Tool for ToolSearchTool {
    fn name(&self) -> &str {
        "ToolSearch"
    }

    fn description(&self) -> &str {
        "Search for deferred tools and load their full schema. \
         Use this before calling any deferred tool."
    }

    fn input_schema(&self) -> JsonSchema {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Tool name or keyword to search for"
                }
            },
            "required": ["query"]
        })
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        true
    }

    async fn execute(&self, input: Value) -> ToolResult {
        let query = input["query"].as_str().unwrap_or("");
        if query.is_empty() {
            return ToolResult {
                content: "Error: query is required".to_string(),
                is_error: true,
            };
        }

        let query_lower = query.to_lowercase();
        let defs = self.registry.to_tool_defs();
        let matches: Vec<Value> = defs
            .iter()
            .filter(|d| d.deferred)
            .filter(|d| {
                d.name.to_lowercase().contains(&query_lower)
                    || d.description.to_lowercase().contains(&query_lower)
            })
            .map(|d| {
                json!({
                    "name": d.name,
                    "description": d.description,
                    "parameters": d.input_schema
                })
            })
            .collect();

        if matches.is_empty() {
            return ToolResult {
                content: format!("No deferred tools matching \"{}\" found.", query),
                is_error: false,
            };
        }

        ToolResult {
            content: serde_json::to_string_pretty(&matches).unwrap_or_default(),
            is_error: false,
        }
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Info
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockDeferred {
        tool_name: String,
        tool_description: String,
        deferred: bool,
    }

    #[async_trait]
    impl Tool for MockDeferred {
        fn name(&self) -> &str { &self.tool_name }
        fn description(&self) -> &str { &self.tool_description }
        fn input_schema(&self) -> JsonSchema {
            json!({"type": "object", "properties": {"x": {"type": "string"}}})
        }
        fn is_concurrency_safe(&self, _input: &Value) -> bool { true }
        fn is_deferred(&self) -> bool { self.deferred }
        async fn execute(&self, _input: Value) -> ToolResult {
            ToolResult { content: "ok".into(), is_error: false }
        }
        fn category(&self) -> ToolCategory { ToolCategory::Info }
    }

    fn build_registry() -> Arc<ToolRegistry> {
        let mut reg = ToolRegistry::new();
        reg.register(Box::new(MockDeferred {
            tool_name: "Read".into(),
            tool_description: "Read a file".into(),
            deferred: false,
        }));
        reg.register(Box::new(MockDeferred {
            tool_name: "SpawnTool".into(),
            tool_description: "Spawn sub-agents".into(),
            deferred: true,
        }));
        reg.register(Box::new(MockDeferred {
            tool_name: "EnterPlanMode".into(),
            tool_description: "Enter plan mode".into(),
            deferred: true,
        }));
        Arc::new(reg)
    }

    #[tokio::test]
    async fn search_by_exact_name() {
        let tool = ToolSearchTool::new(build_registry());
        let result = tool.execute(json!({"query": "SpawnTool"})).await;
        assert!(!result.is_error);
        assert!(result.content.contains("SpawnTool"));
        assert!(result.content.contains("Spawn sub-agents"));
        assert!(result.content.contains("parameters"));
    }

    #[tokio::test]
    async fn search_case_insensitive() {
        let tool = ToolSearchTool::new(build_registry());
        let result = tool.execute(json!({"query": "spawntool"})).await;
        assert!(!result.is_error);
        assert!(result.content.contains("SpawnTool"));
    }

    #[tokio::test]
    async fn search_by_description_keyword() {
        let tool = ToolSearchTool::new(build_registry());
        let result = tool.execute(json!({"query": "plan"})).await;
        assert!(!result.is_error);
        assert!(result.content.contains("EnterPlanMode"));
    }

    #[tokio::test]
    async fn search_excludes_non_deferred() {
        let tool = ToolSearchTool::new(build_registry());
        let result = tool.execute(json!({"query": "Read"})).await;
        // "Read" is not deferred, should not appear in results
        assert!(
            !result.content.contains("\"name\": \"Read\"")
                || result.content.contains("No deferred tools")
        );
    }

    #[tokio::test]
    async fn search_no_match() {
        let tool = ToolSearchTool::new(build_registry());
        let result = tool.execute(json!({"query": "nonexistent"})).await;
        assert!(!result.is_error);
        assert!(result.content.contains("No deferred tools"));
    }

    #[tokio::test]
    async fn search_empty_query_returns_error() {
        let tool = ToolSearchTool::new(build_registry());
        let result = tool.execute(json!({"query": ""})).await;
        assert!(result.is_error);
    }
}
