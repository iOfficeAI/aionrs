pub mod bash;
pub mod edit;
pub mod glob;
pub mod grep;
pub mod orchestration;
pub mod read;
pub mod registry;
pub mod spawn;
pub mod write;

use async_trait::async_trait;
use serde_json::Value;

use crate::protocol::events::ToolCategory;
use crate::types::tool::{JsonSchema, ToolResult};

/// A tool that the agent can invoke
#[async_trait]
pub trait Tool: Send + Sync {
    /// Tool name (must match API schema)
    fn name(&self) -> &str;

    /// Human-readable description for the LLM
    fn description(&self) -> &str;

    /// JSON Schema for input parameters
    fn input_schema(&self) -> JsonSchema;

    /// Whether this tool is safe to run concurrently
    fn is_concurrency_safe(&self, input: &Value) -> bool;

    /// Execute the tool
    async fn execute(&self, input: Value) -> ToolResult;

    /// Max result size in chars before truncation
    fn max_result_size(&self) -> usize {
        50_000
    }

    /// Tool category for protocol classification
    fn category(&self) -> ToolCategory;

    /// Human-readable description of what the tool will do with the given input
    fn describe(&self, input: &Value) -> String {
        format!("{}: {}", self.name(), serde_json::to_string(input).unwrap_or_default())
    }
}
