use aion_types::llm::LlmEvent;

use crate::framing::{Frame, FrameKind};

pub(crate) trait ResponseParser {
    type State;

    fn new_state(&self) -> Self::State;
    fn parse_frame(&self, frame: &Frame, state: &mut Self::State) -> Vec<LlmEvent>;
    fn finish(&self, state: &mut Self::State) -> Vec<LlmEvent>;
}

#[derive(Clone, Copy)]
pub(crate) struct OpenAiParser {
    pub auto_tool_id: bool,
}

#[derive(Clone, Copy)]
pub(crate) struct OpenAiResponsesParser;

impl ResponseParser for OpenAiResponsesParser {
    type State = crate::openai_responses::StreamState;

    fn new_state(&self) -> Self::State {
        crate::openai_responses::StreamState::new()
    }

    fn parse_frame(&self, frame: &Frame, state: &mut Self::State) -> Vec<LlmEvent> {
        match frame.kind {
            FrameKind::Data => crate::openai_responses::parse_sse_chunk(&frame.data, state),
            FrameKind::Done => Vec::new(),
        }
    }

    fn finish(&self, _state: &mut Self::State) -> Vec<LlmEvent> {
        Vec::new()
    }
}

impl ResponseParser for OpenAiParser {
    type State = crate::openai::StreamState;

    fn new_state(&self) -> Self::State {
        crate::openai::StreamState::new()
    }

    fn parse_frame(&self, frame: &Frame, state: &mut Self::State) -> Vec<LlmEvent> {
        match frame.kind {
            FrameKind::Done => state.flush_done().into_iter().collect(),
            FrameKind::Data => crate::openai::parse_sse_chunk(&frame.data, state, self.auto_tool_id),
        }
    }

    fn finish(&self, _state: &mut Self::State) -> Vec<LlmEvent> {
        Vec::new()
    }
}

#[derive(Clone, Copy)]
pub(crate) struct AnthropicParser;

impl ResponseParser for AnthropicParser {
    type State = crate::anthropic_shared::StreamState;

    fn new_state(&self) -> Self::State {
        crate::anthropic_shared::StreamState::new()
    }

    fn parse_frame(&self, frame: &Frame, state: &mut Self::State) -> Vec<LlmEvent> {
        let event_type = frame.event.as_deref().unwrap_or("");
        crate::anthropic_shared::parse_sse_data(event_type, &frame.data, state)
    }

    fn finish(&self, _state: &mut Self::State) -> Vec<LlmEvent> {
        Vec::new()
    }
}

#[cfg(test)]
#[path = "parser_test.rs"]
mod parser_test;
