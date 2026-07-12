use serde_json::Value;

pub(crate) fn parse_schema(schema_json: &str) -> Result<Value, String> {
    serde_json::from_str(schema_json).map_err(|error| error.to_string())
}

pub(crate) fn validation_errors(schema: &Value, instance: &Value) -> Result<Vec<String>, String> {
    let validator = jsonschema::validator_for(schema).map_err(|error| error.to_string())?;
    Ok(validator
        .iter_errors(instance)
        .map(|error| format!("{}: {error}", error.instance_path()))
        .collect())
}
