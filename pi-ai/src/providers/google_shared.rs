use serde_json::{json, Value};

use crate::types::*;

/// Convert internal messages to Google Generative AI Content format.
pub fn convert_messages(context: &Context) -> Vec<Value> {
    let mut contents = Vec::new();

    for msg in &context.messages {
        match msg {
            Message::User(user) => {
                let parts = match &user.content {
                    UserContent::Text(text) => vec![json!({"text": text})],
                    UserContent::Blocks(blocks) => blocks
                        .iter()
                        .map(|b| match b {
                            UserContentBlock::Text(t) => json!({"text": t.text}),
                            UserContentBlock::Image(img) => {
                                json!({
                                    "inlineData": {
                                        "mimeType": img.mime_type,
                                        "data": img.data,
                                    }
                                })
                            }
                        })
                        .collect(),
                };
                contents.push(json!({"role": "user", "parts": parts}));
            }
            Message::Assistant(assistant) => {
                let parts: Vec<Value> = assistant
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        ContentBlock::Text(t) => Some(json!({"text": t.text})),
                        ContentBlock::Thinking(t) => {
                            Some(json!({"text": t.thinking, "thought": true}))
                        }
                        ContentBlock::ToolCall(tc) => {
                            Some(json!({
                                "functionCall": {
                                    "name": tc.name,
                                    "args": tc.arguments,
                                }
                            }))
                        }
                        ContentBlock::Image(_) => None,
                    })
                    .collect();
                if !parts.is_empty() {
                    contents.push(json!({"role": "model", "parts": parts}));
                }
            }
            Message::ToolResult(tr) => {
                let text = tr
                    .content
                    .iter()
                    .filter_map(|c| match c {
                        ToolResultContent::Text(t) => Some(t.text.clone()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");

                contents.push(json!({
                    "role": "user",
                    "parts": [{
                        "functionResponse": {
                            "name": tr.tool_name,
                            "response": { "result": text },
                        }
                    }]
                }));
            }
        }
    }

    contents
}

/// Convert tools to Google format.
pub fn convert_tools(tools: &[Tool]) -> Vec<Value> {
    let declarations: Vec<Value> = tools
        .iter()
        .map(|t| {
            json!({
                "name": t.name,
                "description": t.description,
                "parameters": t.parameters,
            })
        })
        .collect();

    vec![json!({"functionDeclarations": declarations})]
}

/// Map Google finish reason to internal StopReason.
pub fn map_stop_reason(reason: &str) -> StopReason {
    match reason {
        "STOP" => StopReason::Stop,
        "MAX_TOKENS" => StopReason::Length,
        "SAFETY" | "RECITATION" | "OTHER" => StopReason::Stop,
        _ => StopReason::Stop,
    }
}
