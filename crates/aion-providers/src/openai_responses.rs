use std::collections::HashSet;

use aion_types::llm::LlmEvent;
use aion_types::message::{StopReason, TokenUsage};
use serde_json::{Map, Value};

pub(crate) const PROVIDER_ITEM_OWNER: &str = "openai_responses";

pub(crate) struct StreamState {
    seen_output_items: HashSet<String>,
    saw_tool_call: bool,
    terminal: bool,
}

impl StreamState {
    pub(crate) fn new() -> Self {
        Self {
            seen_output_items: HashSet::new(),
            saw_tool_call: false,
            terminal: false,
        }
    }

    pub(crate) fn is_terminal(&self) -> bool {
        self.terminal
    }
}

pub(crate) fn parse_sse_chunk(data: &str, state: &mut StreamState) -> Vec<LlmEvent> {
    let json = match serde_json::from_str::<Value>(data) {
        Ok(json) => json,
        Err(_) => return Vec::new(),
    };
    let event_type = json.get("type").and_then(Value::as_str).unwrap_or_default();

    match event_type {
        "response.output_text.delta" | "response.refusal.delta" => json
            .get("delta")
            .and_then(Value::as_str)
            .filter(|delta| !delta.is_empty())
            .map(|delta| vec![LlmEvent::TextDelta(delta.to_string())])
            .unwrap_or_default(),
        "response.reasoning_summary_text.delta" => json
            .get("delta")
            .and_then(Value::as_str)
            .filter(|delta| !delta.is_empty())
            .map(|delta| vec![LlmEvent::ThinkingDelta(delta.to_string())])
            .unwrap_or_default(),
        "response.output_item.done" => json
            .get("item")
            .map(|item| events_for_output_item(item, state))
            .unwrap_or_default(),
        "response.completed" => complete_response(&json, state),
        "response.incomplete" => incomplete_response(&json, state),
        "response.failed" => failed_response(&json, state),
        "error" => error_event(&json, state),
        _ => Vec::new(),
    }
}

fn complete_response(event: &Value, state: &mut StreamState) -> Vec<LlmEvent> {
    let response = event.get("response").unwrap_or(event);
    let mut events = response
        .get("output")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .flat_map(|item| events_for_output_item(item, state))
        .collect::<Vec<_>>();

    state.terminal = true;
    events.push(LlmEvent::Done {
        stop_reason: if state.saw_tool_call {
            StopReason::ToolUse
        } else {
            StopReason::EndTurn
        },
        usage: response.get("usage").map(parse_usage).unwrap_or_default(),
    });
    events
}

fn incomplete_response(event: &Value, state: &mut StreamState) -> Vec<LlmEvent> {
    let response = event.get("response").unwrap_or(event);
    let mut events = response
        .get("output")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .flat_map(|item| events_for_output_item(item, state))
        .collect::<Vec<_>>();
    let reason = response
        .pointer("/incomplete_details/reason")
        .and_then(Value::as_str)
        .unwrap_or("unknown");

    state.terminal = true;
    if reason == "max_output_tokens" {
        events.push(LlmEvent::Done {
            stop_reason: StopReason::MaxTokens,
            usage: response.get("usage").map(parse_usage).unwrap_or_default(),
        });
    } else {
        events.push(LlmEvent::Error(format!("OpenAI response incomplete: {reason}")));
    }
    events
}

fn failed_response(event: &Value, state: &mut StreamState) -> Vec<LlmEvent> {
    state.terminal = true;
    let response = event.get("response").unwrap_or(event);
    let message = response
        .pointer("/error/message")
        .and_then(Value::as_str)
        .or_else(|| response.get("error").and_then(Value::as_str))
        .unwrap_or("OpenAI response failed");
    vec![LlmEvent::Error(message.to_string())]
}

fn error_event(event: &Value, state: &mut StreamState) -> Vec<LlmEvent> {
    state.terminal = true;
    let message = event
        .pointer("/error/message")
        .and_then(Value::as_str)
        .or_else(|| event.get("message").and_then(Value::as_str))
        .unwrap_or("OpenAI Responses stream error");
    vec![LlmEvent::Error(message.to_string())]
}

fn events_for_output_item(item: &Value, state: &mut StreamState) -> Vec<LlmEvent> {
    let identity = item
        .get("id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| item.to_string());
    if !state.seen_output_items.insert(identity) {
        return Vec::new();
    }

    match item.get("type").and_then(Value::as_str) {
        Some("reasoning") => vec![LlmEvent::ProviderItem {
            provider: PROVIDER_ITEM_OWNER.to_string(),
            item: item.clone(),
        }],
        Some("function_call") => function_call_event(item, state).into_iter().collect(),
        _ => Vec::new(),
    }
}

fn function_call_event(item: &Value, state: &mut StreamState) -> Option<LlmEvent> {
    let call_id = item.get("call_id").and_then(Value::as_str)?.to_string();
    let name = item.get("name").and_then(Value::as_str).unwrap_or_default().to_string();
    let arguments = item.get("arguments").and_then(Value::as_str).unwrap_or("{}");
    let input = serde_json::from_str(arguments).unwrap_or_else(|_| Value::Object(Map::new()));

    let mut provider_metadata = Map::new();
    let mut function_call = Map::new();
    function_call.insert("function_call".to_string(), item.clone());
    provider_metadata.insert(PROVIDER_ITEM_OWNER.to_string(), Value::Object(function_call));

    state.saw_tool_call = true;
    Some(LlmEvent::ToolUse {
        id: call_id,
        name,
        input,
        extra: Some(Value::Object(provider_metadata)),
    })
}

fn parse_usage(usage: &Value) -> TokenUsage {
    TokenUsage {
        input_tokens: usage.get("input_tokens").and_then(Value::as_u64).unwrap_or(0),
        output_tokens: usage.get("output_tokens").and_then(Value::as_u64).unwrap_or(0),
        cache_creation_tokens: 0,
        cache_read_tokens: usage
            .pointer("/input_tokens_details/cached_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
    }
}

#[cfg(test)]
#[path = "openai_responses_test.rs"]
mod openai_responses_test;
