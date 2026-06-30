use super::*;

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
