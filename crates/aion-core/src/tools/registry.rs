use crate::types::tool::ToolDef;

use super::Tool;

pub struct ToolRegistry {
    tools: Vec<Box<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self { tools: Vec::new() }
    }

    pub fn register(&mut self, tool: Box<dyn Tool>) {
        self.tools.push(tool);
    }

    /// Find a tool by name
    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.tools
            .iter()
            .find(|t| t.name() == name)
            .map(|t| t.as_ref())
    }

    /// Get all registered tool names
    pub fn tool_names(&self) -> Vec<String> {
        self.tools.iter().map(|t| t.name().to_string()).collect()
    }

    /// Generate API tool definitions for all registered tools
    pub fn to_tool_defs(&self) -> Vec<ToolDef> {
        self.tools
            .iter()
            .map(|t| ToolDef {
                name: t.name().to_string(),
                description: t.description().to_string(),
                input_schema: t.input_schema(),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use crate::protocol::events::ToolCategory;
    use crate::tools::Tool;
    use crate::types::tool::ToolResult;

    /// A minimal Tool implementation used only in tests
    struct MockTool {
        tool_name: String,
        tool_description: String,
    }

    #[async_trait]
    impl Tool for MockTool {
        fn name(&self) -> &str {
            &self.tool_name
        }

        fn description(&self) -> &str {
            &self.tool_description
        }

        fn input_schema(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }

        fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
            true
        }

        async fn execute(&self, _input: serde_json::Value) -> ToolResult {
            ToolResult { content: "ok".to_string(), is_error: false }
        }

        fn category(&self) -> ToolCategory {
            ToolCategory::Info
        }
    }

    /// Helper to create a MockTool with the given name and description
    fn make_tool(name: &str, description: &str) -> Box<MockTool> {
        Box::new(MockTool {
            tool_name: name.to_string(),
            tool_description: description.to_string(),
        })
    }

    #[test]
    fn test_register_and_get() {
        let mut registry = ToolRegistry::new();
        registry.register(make_tool("my_tool", "does something"));

        let found = registry.get("my_tool");
        assert!(found.is_some(), "registered tool should be retrievable by name");
        assert_eq!(found.unwrap().name(), "my_tool");
    }

    #[test]
    fn test_get_nonexistent_returns_none() {
        let registry = ToolRegistry::new();

        let result = registry.get("ghost");
        assert!(result.is_none(), "looking up an unregistered name should return None");
    }

    #[test]
    fn test_tool_names() {
        let mut registry = ToolRegistry::new();
        registry.register(make_tool("alpha", "first tool"));
        registry.register(make_tool("beta", "second tool"));
        registry.register(make_tool("gamma", "third tool"));

        let mut names = registry.tool_names();
        names.sort(); // sort for a stable assertion order
        assert_eq!(names, vec!["alpha", "beta", "gamma"]);
    }

    #[test]
    fn test_to_tool_defs() {
        let mut registry = ToolRegistry::new();
        registry.register(make_tool("tool_a", "description A"));
        registry.register(make_tool("tool_b", "description B"));

        let defs = registry.to_tool_defs();
        assert_eq!(defs.len(), 2, "to_tool_defs should return one entry per registered tool");

        // Collect (name, description) pairs for assertion independent of order
        let mut pairs: Vec<(&str, &str)> = defs
            .iter()
            .map(|d| (d.name.as_str(), d.description.as_str()))
            .collect();
        pairs.sort();

        assert_eq!(pairs[0], ("tool_a", "description A"));
        assert_eq!(pairs[1], ("tool_b", "description B"));

        // Verify the input_schema field is populated correctly
        let expected_schema = serde_json::json!({"type": "object"});
        for def in &defs {
            assert_eq!(def.input_schema, expected_schema);
        }
    }
}
