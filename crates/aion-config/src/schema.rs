use serde_json::{Map, Value};

pub const JSON_SCHEMA_DRAFT_2020_12: &str = "https://json-schema.org/draft/2020-12/schema";

/// Apply provider-neutral JSON Schema fixes required for tool declarations.
pub fn legalize_json_schema(schema: &Value) -> Value {
    match schema {
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) | Value::Array(_) => {
            empty_object_schema()
        }
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

                if object.get("type").and_then(Value::as_str) == Some("object")
                    && !object.contains_key("properties")
                {
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
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn legalize_json_schema_null_becomes_empty_object_schema() {
        assert_eq!(
            legalize_json_schema(&Value::Null),
            json!({
                "$schema": JSON_SCHEMA_DRAFT_2020_12,
                "type": "object",
                "properties": {}
            })
        );
    }

    #[test]
    fn legalize_json_schema_empty_object_becomes_empty_object_schema() {
        assert_eq!(
            legalize_json_schema(&json!({})),
            json!({
                "$schema": JSON_SCHEMA_DRAFT_2020_12,
                "type": "object",
                "properties": {}
            })
        );
    }

    #[test]
    fn legalize_json_schema_boolean_roots_become_empty_object_schema() {
        for schema in [json!(true), json!(false)] {
            assert_eq!(
                legalize_json_schema(&schema),
                json!({
                    "$schema": JSON_SCHEMA_DRAFT_2020_12,
                    "type": "object",
                    "properties": {}
                })
            );
        }
    }

    #[test]
    fn legalize_json_schema_string_array_and_number_roots_become_empty_object_schema() {
        for schema in [json!("raw"), json!(["not", "object"]), json!(42)] {
            assert_eq!(
                legalize_json_schema(&schema),
                json!({
                    "$schema": JSON_SCHEMA_DRAFT_2020_12,
                    "type": "object",
                    "properties": {}
                })
            );
        }
    }

    #[test]
    fn legalize_json_schema_non_object_root_type_becomes_empty_object_schema() {
        assert_eq!(
            legalize_json_schema(&json!({
                "type": "string"
            })),
            json!({
                "$schema": JSON_SCHEMA_DRAFT_2020_12,
                "type": "object",
                "properties": {}
            })
        );
    }

    #[test]
    fn legalize_json_schema_missing_root_type_defaults_to_object() {
        let legalized = legalize_json_schema(&json!({
            "properties": {
                "path": { "type": "string" }
            }
        }));

        assert_eq!(legalized["type"], "object");
        assert_eq!(legalized["properties"]["path"]["type"], "string");
    }

    #[test]
    fn legalize_json_schema_missing_root_properties_defaults_to_empty_object() {
        let legalized = legalize_json_schema(&json!({
            "type": "object"
        }));

        assert_eq!(legalized["properties"], json!({}));
    }

    #[test]
    fn legalize_json_schema_sets_root_draft_2020_12_schema() {
        let legalized = legalize_json_schema(&json!({
            "type": "object",
            "properties": {}
        }));

        assert_eq!(legalized["$schema"], JSON_SCHEMA_DRAFT_2020_12);
    }
}
