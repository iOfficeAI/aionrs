use super::*;

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use aion_providers::{LlmProvider, ProviderError};
    use aion_types::llm::{LlmEvent, LlmRequest};
    use aion_types::message::{ContentBlock, Message, Role};

    use super::*;
    use crate::commands::{CommandContext, CommandRegistry};
    use crate::compact::state::CompactState;
    use crate::output::null_sink::NullSink;

    struct NullProvider;
    #[async_trait::async_trait]
    impl LlmProvider for NullProvider {
        async fn stream(&self, _: &LlmRequest) -> Result<tokio::sync::mpsc::Receiver<LlmEvent>, ProviderError> {
            let (_tx, rx) = tokio::sync::mpsc::channel(1);
            Ok(rx)
        }
    }

    #[tokio::test]
    async fn compact_already_compact_guard() {
        let provider: Arc<dyn LlmProvider> = Arc::new(NullProvider);
        let registry = CommandRegistry::new();
        let output = NullSink;
        let mut messages = vec![Message::new(Role::User, vec![ContentBlock::Text { text: "hi".into() }])];
        let mut state = CompactState::new();
        let config = aion_config::compact::CompactConfig::default();

        let mut ctx = CommandContext {
            messages: &mut messages,
            compact_state: &mut state,
            compact_config: &config,
            provider,
            model: "test-model",
            output: &output,
            registry: &registry,
        };

        let cmd = CompactCommand;
        let result = cmd.execute(&mut ctx, "").await.unwrap();
        assert_eq!(result, CommandResult::Continue);
        assert_eq!(ctx.messages.len(), 1);
    }

    #[tokio::test]
    async fn compact_resets_circuit_breaker() {
        let provider: Arc<dyn LlmProvider> = Arc::new(NullProvider);
        let registry = CommandRegistry::new();
        let output = NullSink;
        let mut messages: Vec<Message> = (0..10)
            .map(|i| {
                let role = if i % 2 == 0 { Role::User } else { Role::Assistant };
                Message::new(
                    role,
                    vec![ContentBlock::Text {
                        text: format!("msg-{i}"),
                    }],
                )
            })
            .collect();
        let mut state = CompactState::new();
        state.consecutive_failures = 5;
        let config = aion_config::compact::CompactConfig::default();

        let mut ctx = CommandContext {
            messages: &mut messages,
            compact_state: &mut state,
            compact_config: &config,
            provider,
            model: "test-model",
            output: &output,
            registry: &registry,
        };

        let cmd = CompactCommand;
        let _ = cmd.execute(&mut ctx, "").await;
        // Circuit breaker was reset to 0 before the call, then failure increments it
        assert!(ctx.compact_state.consecutive_failures <= 1);
    }
}
