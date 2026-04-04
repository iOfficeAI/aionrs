pub mod bash;
pub mod edit;
pub mod glob;
pub mod grep;
pub mod read;
pub mod registry;
pub mod write;

use async_trait::async_trait;
use serde_json::Value;

use aion_protocol::events::ToolCategory;
use aion_types::tool::{JsonSchema, ToolResult};

/// A tool that the agent can invoke
#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn input_schema(&self) -> JsonSchema;
    fn is_concurrency_safe(&self, input: &Value) -> bool;
    async fn execute(&self, input: Value) -> ToolResult;
    fn max_result_size(&self) -> usize { 50_000 }
    fn category(&self) -> ToolCategory;
    fn describe(&self, input: &Value) -> String {
        format!("{}: {}", self.name(), serde_json::to_string(input).unwrap_or_default())
    }
}
