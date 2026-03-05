use serde_json::Value;

use crate::error::PiAiError;
use crate::types::{Tool, ToolCall};

/// Finds a tool by name and validates the tool call arguments against its JSON Schema.
pub fn validate_tool_call(tools: &[Tool], tool_call: &ToolCall) -> Result<Value, PiAiError> {
    let tool = tools
        .iter()
        .find(|t| t.name == tool_call.name)
        .ok_or_else(|| PiAiError::ToolNotFound(tool_call.name.clone()))?;

    validate_tool_arguments(tool, tool_call)
}

/// Validates tool call arguments against the tool's JSON Schema.
pub fn validate_tool_arguments(tool: &Tool, tool_call: &ToolCall) -> Result<Value, PiAiError> {
    let args_value = serde_json::to_value(&tool_call.arguments)
        .map_err(|e| PiAiError::Validation(format!("Failed to serialize arguments: {e}")))?;

    let schema = &tool.parameters;

    let validator = match jsonschema::validator_for(schema) {
        Ok(v) => v,
        Err(e) => {
            return Err(PiAiError::Validation(format!(
                "Invalid schema for tool \"{}\": {e}",
                tool_call.name
            )));
        }
    };

    let result = validator.validate(&args_value);

    if result.is_ok() {
        Ok(args_value)
    } else {
        let errors: Vec<String> = validator
            .iter_errors(&args_value)
            .map(|err| {
                let path = err.instance_path.to_string();
                let path = if path.is_empty() {
                    "root".to_string()
                } else {
                    path
                };
                format!("  - {path}: {err}")
            })
            .collect();

        let error_message = format!(
            "Validation failed for tool \"{}\":\n{}\n\nReceived arguments:\n{}",
            tool_call.name,
            errors.join("\n"),
            serde_json::to_string_pretty(&tool_call.arguments).unwrap_or_default()
        );

        Err(PiAiError::Validation(error_message))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_validate_valid_args() {
        let tool = Tool {
            name: "test".to_string(),
            description: "test tool".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string" }
                },
                "required": ["name"]
            }),
        };

        let mut args = HashMap::new();
        args.insert(
            "name".to_string(),
            serde_json::Value::String("hello".to_string()),
        );

        let tool_call = ToolCall {
            id: "tc1".to_string(),
            name: "test".to_string(),
            arguments: args,
            thought_signature: None,
        };

        let result = validate_tool_arguments(&tool, &tool_call);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_missing_required() {
        let tool = Tool {
            name: "test".to_string(),
            description: "test tool".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string" }
                },
                "required": ["name"]
            }),
        };

        let tool_call = ToolCall {
            id: "tc1".to_string(),
            name: "test".to_string(),
            arguments: HashMap::new(),
            thought_signature: None,
        };

        let result = validate_tool_arguments(&tool, &tool_call);
        assert!(result.is_err());
    }
}
