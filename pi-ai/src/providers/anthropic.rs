use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::api_registry::ApiProvider;
use crate::env_api_keys::get_env_api_key;
use crate::event_stream::{AssistantMessageEventSender, AssistantMessageEventStream};
use crate::models::calculate_cost;
use crate::types::*;
use crate::utils::simple_options::{adjust_max_tokens_for_thinking, build_base_options};

pub struct AnthropicProvider;

#[async_trait]
impl ApiProvider for AnthropicProvider {
    fn api(&self) -> &str {
        "anthropic-messages"
    }

    fn stream(
        &self,
        model: &Model,
        context: &Context,
        options: &StreamOptions,
    ) -> (AssistantMessageEventStream, AssistantMessageEventSender) {
        let (stream, sender) = AssistantMessageEventStream::new();
        let model = model.clone();
        let context_clone = context.clone();
        let options = options.clone();

        tokio::spawn(async move {
            stream_anthropic(&model, &context_clone, &options, None, sender).await;
        });

        (stream, sender_placeholder())
    }

    fn stream_simple(
        &self,
        model: &Model,
        context: &Context,
        options: &SimpleStreamOptions,
    ) -> (AssistantMessageEventStream, AssistantMessageEventSender) {
        let (stream, sender) = AssistantMessageEventStream::new();
        let model = model.clone();
        let context_clone = context.clone();
        let base_options = build_base_options(&model, options);
        let reasoning = options.reasoning;
        let thinking_budgets = options.thinking_budgets.clone();

        let simple_opts = SimpleStreamOptions {
            base: base_options,
            reasoning,
            thinking_budgets,
        };

        tokio::spawn(async move {
            let (max_tokens, thinking_budget) =
                adjust_max_tokens_for_thinking(&model, &simple_opts);
            let mut opts = simple_opts.base.clone();
            opts.max_tokens = Some(max_tokens);

            stream_anthropic(&model, &context_clone, &opts, Some(thinking_budget), sender).await;
        });

        (stream, sender_placeholder())
    }
}

/// Placeholder sender - the real sender is captured by the spawned task.
fn sender_placeholder() -> AssistantMessageEventSender {
    // Create a dummy that won't be used - the real sender is in the spawned task
    let (_, s) = AssistantMessageEventStream::new();
    s
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

async fn stream_anthropic(
    model: &Model,
    context: &Context,
    options: &StreamOptions,
    thinking_budget: Option<u32>,
    sender: AssistantMessageEventSender,
) {
    let api_key = options
        .api_key
        .clone()
        .or_else(|| get_env_api_key(&model.provider));

    let api_key = match api_key {
        Some(k) => k,
        None => {
            let mut msg = AssistantMessage::default();
            msg.api = "anthropic-messages".to_string();
            msg.provider = model.provider.clone();
            msg.model = model.id.clone();
            msg.stop_reason = StopReason::Error;
            msg.error_message = Some(format!(
                "No API key found for provider '{}'. Set ANTHROPIC_API_KEY.",
                model.provider
            ));
            msg.timestamp = now_ms();
            sender.error(msg);
            return;
        }
    };

    let base_url = &model.base_url;
    let url = format!("{base_url}/v1/messages");

    // Build messages
    let messages = convert_messages(&context.messages);

    // Build request body
    let mut body = json!({
        "model": model.id,
        "messages": messages,
        "max_tokens": options.max_tokens.unwrap_or(model.max_tokens.min(32000)),
        "stream": true,
    });

    if let Some(ref system_prompt) = context.system_prompt {
        body["system"] = json!([{
            "type": "text",
            "text": system_prompt,
            "cache_control": { "type": "ephemeral" }
        }]);
    }

    if let Some(temp) = options.temperature {
        body["temperature"] = json!(temp);
    }

    if let Some(budget) = thinking_budget {
        body["thinking"] = json!({
            "type": "enabled",
            "budget_tokens": budget
        });
    }

    // Add tools
    if let Some(ref tools) = context.tools {
        let tool_defs: Vec<Value> = tools
            .iter()
            .map(|t| {
                json!({
                    "name": t.name,
                    "description": t.description,
                    "input_schema": t.parameters,
                })
            })
            .collect();
        if !tool_defs.is_empty() {
            body["tools"] = json!(tool_defs);
        }
    }

    let client = Client::new();
    let mut request = client
        .post(&url)
        .header("x-api-key", &api_key)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json");

    if thinking_budget.is_some() {
        request = request.header("anthropic-beta", "interleaved-thinking-2025-05-14");
    }

    // Add custom headers
    if let Some(ref headers) = options.headers {
        for (k, v) in headers {
            request = request.header(k, v);
        }
    }

    let response = match request.json(&body).send().await {
        Ok(r) => r,
        Err(e) => {
            let mut msg = AssistantMessage::default();
            msg.api = "anthropic-messages".to_string();
            msg.provider = model.provider.clone();
            msg.model = model.id.clone();
            msg.stop_reason = StopReason::Error;
            msg.error_message = Some(format!("HTTP request failed: {e}"));
            msg.timestamp = now_ms();
            sender.error(msg);
            return;
        }
    };

    if !response.status().is_success() {
        let status = response.status();
        let body_text = response.text().await.unwrap_or_default();
        let mut msg = AssistantMessage::default();
        msg.api = "anthropic-messages".to_string();
        msg.provider = model.provider.clone();
        msg.model = model.id.clone();
        msg.stop_reason = StopReason::Error;
        msg.error_message = Some(format!("{status} {body_text}"));
        msg.timestamp = now_ms();
        sender.error(msg);
        return;
    }

    // Parse SSE stream
    let mut partial = AssistantMessage {
        api: "anthropic-messages".to_string(),
        provider: model.provider.clone(),
        model: model.id.clone(),
        timestamp: now_ms(),
        ..Default::default()
    };

    sender.push(AssistantMessageEvent::Start {
        partial: partial.clone(),
    });

    let mut current_text = String::new();
    let mut current_thinking = String::new();
    let mut current_tool_json = String::new();
    let mut current_tool_id = String::new();
    let mut current_tool_name = String::new();
    let mut content_index: usize = 0;

    // Read SSE events line by line
    let body_stream = response.text().await.unwrap_or_default();
    for line in body_stream.lines() {
        let line = line.trim();
        if !line.starts_with("data: ") {
            continue;
        }
        let data = &line[6..];
        if data == "[DONE]" {
            break;
        }

        let event: Value = match serde_json::from_str(data) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let event_type = event["type"].as_str().unwrap_or("");

        match event_type {
            "message_start" => {
                if let Some(usage) = event.get("message").and_then(|m| m.get("usage")) {
                    if let Some(input) = usage["input_tokens"].as_u64() {
                        partial.usage.input = input;
                    }
                    if let Some(cache_read) = usage["cache_read_input_tokens"].as_u64() {
                        partial.usage.cache_read = cache_read;
                    }
                    if let Some(cache_write) = usage["cache_creation_input_tokens"].as_u64() {
                        partial.usage.cache_write = cache_write;
                    }
                }
            }

            "content_block_start" => {
                content_index = event["index"].as_u64().unwrap_or(0) as usize;
                let block_type = event
                    .get("content_block")
                    .and_then(|b| b["type"].as_str())
                    .unwrap_or("");

                match block_type {
                    "text" => {
                        current_text.clear();
                        sender.push(AssistantMessageEvent::TextStart {
                            content_index,
                            partial: partial.clone(),
                        });
                    }
                    "thinking" => {
                        current_thinking.clear();
                        sender.push(AssistantMessageEvent::ThinkingStart {
                            content_index,
                            partial: partial.clone(),
                        });
                    }
                    "tool_use" => {
                        current_tool_json.clear();
                        current_tool_id = event
                            .get("content_block")
                            .and_then(|b| b["id"].as_str())
                            .unwrap_or("")
                            .to_string();
                        current_tool_name = event
                            .get("content_block")
                            .and_then(|b| b["name"].as_str())
                            .unwrap_or("")
                            .to_string();
                        sender.push(AssistantMessageEvent::ToolCallStart {
                            content_index,
                            partial: partial.clone(),
                        });
                    }
                    _ => {}
                }
            }

            "content_block_delta" => {
                let delta_type = event
                    .get("delta")
                    .and_then(|d| d["type"].as_str())
                    .unwrap_or("");

                match delta_type {
                    "text_delta" => {
                        let text = event
                            .get("delta")
                            .and_then(|d| d["text"].as_str())
                            .unwrap_or("");
                        current_text.push_str(text);
                        sender.push(AssistantMessageEvent::TextDelta {
                            content_index,
                            delta: text.to_string(),
                            partial: partial.clone(),
                        });
                    }
                    "thinking_delta" => {
                        let thinking = event
                            .get("delta")
                            .and_then(|d| d["thinking"].as_str())
                            .unwrap_or("");
                        current_thinking.push_str(thinking);
                        sender.push(AssistantMessageEvent::ThinkingDelta {
                            content_index,
                            delta: thinking.to_string(),
                            partial: partial.clone(),
                        });
                    }
                    "input_json_delta" => {
                        let json_delta = event
                            .get("delta")
                            .and_then(|d| d["partial_json"].as_str())
                            .unwrap_or("");
                        current_tool_json.push_str(json_delta);
                        sender.push(AssistantMessageEvent::ToolCallDelta {
                            content_index,
                            delta: json_delta.to_string(),
                            partial: partial.clone(),
                        });
                    }
                    _ => {}
                }
            }

            "content_block_stop" => {
                // Determine what type of block just ended based on what we've accumulated
                if !current_tool_name.is_empty() {
                    let arguments: HashMap<String, Value> =
                        serde_json::from_str(&current_tool_json).unwrap_or_default();
                    let tool_call = ToolCall {
                        id: std::mem::take(&mut current_tool_id),
                        name: std::mem::take(&mut current_tool_name),
                        arguments,
                        thought_signature: None,
                    };
                    partial
                        .content
                        .push(ContentBlock::ToolCall(tool_call.clone()));
                    sender.push(AssistantMessageEvent::ToolCallEnd {
                        content_index,
                        tool_call,
                        partial: partial.clone(),
                    });
                    current_tool_json.clear();
                } else if !current_thinking.is_empty() {
                    let content = std::mem::take(&mut current_thinking);
                    partial
                        .content
                        .push(ContentBlock::Thinking(ThinkingContent {
                            thinking: content.clone(),
                            thinking_signature: None,
                            redacted: None,
                        }));
                    sender.push(AssistantMessageEvent::ThinkingEnd {
                        content_index,
                        content,
                        partial: partial.clone(),
                    });
                } else {
                    let content = std::mem::take(&mut current_text);
                    partial.content.push(ContentBlock::Text(TextContent {
                        text: content.clone(),
                        text_signature: None,
                    }));
                    sender.push(AssistantMessageEvent::TextEnd {
                        content_index,
                        content,
                        partial: partial.clone(),
                    });
                }
            }

            "message_delta" => {
                if let Some(stop_reason) = event.get("delta").and_then(|d| d["stop_reason"].as_str()) {
                    partial.stop_reason = match stop_reason {
                        "end_turn" | "stop" => StopReason::Stop,
                        "max_tokens" => StopReason::Length,
                        "tool_use" => StopReason::ToolUse,
                        _ => StopReason::Stop,
                    };
                }
                if let Some(usage) = event.get("usage") {
                    if let Some(output) = usage["output_tokens"].as_u64() {
                        partial.usage.output = output;
                    }
                }
            }

            "message_stop" => {
                // Final
            }

            "error" => {
                let error_msg = event
                    .get("error")
                    .and_then(|e| e["message"].as_str())
                    .unwrap_or("Unknown error");
                partial.stop_reason = StopReason::Error;
                partial.error_message = Some(error_msg.to_string());
            }

            _ => {}
        }
    }

    // Calculate cost and finalize
    partial.usage.total_tokens = partial.usage.input
        + partial.usage.output
        + partial.usage.cache_read
        + partial.usage.cache_write;
    calculate_cost(model, &mut partial.usage);

    sender.finish(partial);
}

/// Convert internal messages to Anthropic API format.
fn convert_messages(messages: &[Message]) -> Vec<Value> {
    let mut result = Vec::new();

    for msg in messages {
        match msg {
            Message::User(user) => {
                let content = match &user.content {
                    UserContent::Text(text) => json!(text),
                    UserContent::Blocks(blocks) => {
                        let converted: Vec<Value> = blocks
                            .iter()
                            .map(|b| match b {
                                UserContentBlock::Text(t) => {
                                    json!({"type": "text", "text": t.text})
                                }
                                UserContentBlock::Image(img) => {
                                    json!({
                                        "type": "image",
                                        "source": {
                                            "type": "base64",
                                            "media_type": img.mime_type,
                                            "data": img.data,
                                        }
                                    })
                                }
                            })
                            .collect();
                        json!(converted)
                    }
                };
                result.push(json!({"role": "user", "content": content}));
            }
            Message::Assistant(assistant) => {
                let content: Vec<Value> = assistant
                    .content
                    .iter()
                    .map(|block| match block {
                        ContentBlock::Text(t) => json!({"type": "text", "text": t.text}),
                        ContentBlock::Thinking(t) => {
                            json!({"type": "thinking", "thinking": t.thinking})
                        }
                        ContentBlock::ToolCall(tc) => {
                            json!({
                                "type": "tool_use",
                                "id": tc.id,
                                "name": tc.name,
                                "input": tc.arguments,
                            })
                        }
                        ContentBlock::Image(_) => json!({"type": "text", "text": "[image]"}),
                    })
                    .collect();
                result.push(json!({"role": "assistant", "content": content}));
            }
            Message::ToolResult(tr) => {
                let content: Vec<Value> = tr
                    .content
                    .iter()
                    .map(|c| match c {
                        ToolResultContent::Text(t) => json!({"type": "text", "text": t.text}),
                        ToolResultContent::Image(img) => {
                            json!({
                                "type": "image",
                                "source": {
                                    "type": "base64",
                                    "media_type": img.mime_type,
                                    "data": img.data,
                                }
                            })
                        }
                    })
                    .collect();
                result.push(json!({
                    "role": "user",
                    "content": [{
                        "type": "tool_result",
                        "tool_use_id": tr.tool_call_id,
                        "content": content,
                        "is_error": tr.is_error,
                    }]
                }));
            }
        }
    }

    result
}
