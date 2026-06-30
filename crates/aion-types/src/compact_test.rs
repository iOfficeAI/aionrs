use super::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trigger_auto_serializes_to_snake_case() {
        let json = serde_json::to_string(&CompactTrigger::Auto).unwrap();
        assert_eq!(json, "\"auto\"");
    }

    #[test]
    fn trigger_manual_serializes_to_snake_case() {
        let json = serde_json::to_string(&CompactTrigger::Manual).unwrap();
        assert_eq!(json, "\"manual\"");
    }

    #[test]
    fn trigger_roundtrip() {
        for trigger in [CompactTrigger::Auto, CompactTrigger::Manual] {
            let json = serde_json::to_value(trigger).unwrap();
            let back: CompactTrigger = serde_json::from_value(json).unwrap();
            assert_eq!(back, trigger);
        }
    }

    #[test]
    fn metadata_serialization_roundtrip() {
        let meta = CompactMetadata {
            trigger: CompactTrigger::Auto,
            pre_compact_tokens: 150_000,
            messages_summarized: 42,
        };
        let json = serde_json::to_value(&meta).unwrap();
        let back: CompactMetadata = serde_json::from_value(json).unwrap();
        assert_eq!(back, meta);
    }

    #[test]
    fn metadata_json_field_names() {
        let meta = CompactMetadata {
            trigger: CompactTrigger::Manual,
            pre_compact_tokens: 200_000,
            messages_summarized: 10,
        };
        let json = serde_json::to_value(&meta).unwrap();
        assert_eq!(json["trigger"], "manual");
        assert_eq!(json["pre_compact_tokens"], 200_000);
        assert_eq!(json["messages_summarized"], 10);
    }
}
