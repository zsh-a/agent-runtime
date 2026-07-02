use serde_json::{Value, json};

use crate::types::{LlmError, LlmResponseFormat};

pub(crate) fn structured_output_from_content(
    format: &Option<LlmResponseFormat>,
    content: &str,
) -> Result<Option<Value>, LlmError> {
    let Some(format) = format else {
        return Ok(None);
    };

    let value = serde_json::from_str::<Value>(content).map_err(|err| {
        LlmError::provider(
            "structured_output_parse_failed",
            format!("model response did not parse as JSON: {err}"),
            false,
            json!({"content": content}),
        )
    })?;

    if matches!(format, LlmResponseFormat::JsonObject) && !value.is_object() {
        return Err(LlmError::provider(
            "structured_output_not_object",
            "model response JSON must be an object",
            false,
            json!({"output": value}),
        ));
    }

    if let LlmResponseFormat::JsonSchema { schema, .. } = format {
        let validator = jsonschema::validator_for(schema).map_err(|err| {
            LlmError::validation(format!("response JSON schema is invalid: {err}"))
        })?;
        let errors = validator
            .iter_errors(&value)
            .map(|error| error.to_string())
            .collect::<Vec<_>>();
        if !errors.is_empty() {
            return Err(LlmError::provider(
                "structured_output_schema_validation_failed",
                "model response JSON did not match response schema",
                false,
                json!({"errors": errors, "output": value}),
            ));
        }
    }

    Ok(Some(value))
}

pub(crate) fn structured_output_instruction(format: &Option<LlmResponseFormat>) -> Option<String> {
    match format {
        None => None,
        Some(LlmResponseFormat::JsonObject) => Some(
            "Respond with one valid JSON object only. Do not include markdown fences or explanatory text."
                .to_owned(),
        ),
        Some(LlmResponseFormat::JsonSchema {
            name,
            schema,
            strict,
        }) => Some(format!(
            "Respond with one valid JSON object only for schema `{name}`. Do not include markdown fences or explanatory text. Strict mode: {}. JSON Schema: {}",
            strict.unwrap_or(true),
            schema
        )),
    }
}
