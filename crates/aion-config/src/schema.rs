use serde_json::{Map, Value};

pub const JSON_SCHEMA_DRAFT_2020_12: &str = "https://json-schema.org/draft/2020-12/schema";

/// Apply provider-neutral JSON Schema fixes required for tool declarations.
pub fn legalize_json_schema(schema: &Value) -> Value {
    match schema {
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) | Value::Array(_) => empty_object_schema(),
        Value::Object(object) if object.is_empty() => empty_object_schema(),
        Value::Object(_) => {
            if has_non_object_root_type(schema) {
                return empty_object_schema();
            }

            let mut legalized = schema.clone();
            if let Some(object) = legalized.as_object_mut() {
                if !object.contains_key("type") {
                    object.insert("type".to_string(), Value::String("object".to_string()));
                }

                if object.get("type").and_then(Value::as_str) == Some("object") && !object.contains_key("properties") {
                    object.insert("properties".to_string(), Value::Object(Map::new()));
                }

                object.insert(
                    "$schema".to_string(),
                    Value::String(JSON_SCHEMA_DRAFT_2020_12.to_string()),
                );
            }

            legalized
        }
    }
}

fn has_non_object_root_type(schema: &Value) -> bool {
    match schema.get("type") {
        Some(Value::String(root_type)) => root_type != "object",
        Some(_) => true,
        None => false,
    }
}

fn empty_object_schema() -> Value {
    let mut object = Map::new();
    object.insert(
        "$schema".to_string(),
        Value::String(JSON_SCHEMA_DRAFT_2020_12.to_string()),
    );
    object.insert("type".to_string(), Value::String("object".to_string()));
    object.insert("properties".to_string(), Value::Object(Map::new()));
    Value::Object(object)
}

#[cfg(test)]
#[path = "schema_test.rs"]
mod schema_test;
