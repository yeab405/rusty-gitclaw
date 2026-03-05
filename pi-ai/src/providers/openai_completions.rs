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
use crate::utils::simple_options::build_base_options;

pub struct OpenAICompletionsProvider;

#[async_trait]
impl ApiProvider for OpenAICompletionsProvider {
    fn api(&self) -> &str {
        "openai-completions"
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
            stream_openai(&model, &context_clone, &options, None, sender).await;
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

        tokio::spawn(async move {
            let reasoning_effort = reasoning.map(|r| match r {
                ThinkingLevel::Minimal | ThinkingLevel::Low => "low",
                ThinkingLevel::Medium => "medium",
                ThinkingLevel::High | ThinkingLevel::Xhigh => "high",
            });
            stream_openai(&model, &context_clone, &base_options, reasoning_effort.map(String::from), sender).await;
        });

        (stream, sender_placeholder())
    }
}

fn sender_placeholder() -> AssistantMessageEventSender {
    let (_, s) = AssistantMessageEventStream::new();
    s
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

async fn stream_openai(
    model: &Model,
    context: &Context,
    options: &StreamOptions,
    reasoning_effort: Option<String>,
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
            msg.api = "openai-completions".to_string();
            msg.provider = model.provider.clone();
            msg.model = model.id.clone();
            msg.stop_reason = StopReason::Error;
            msg.error_message = Some(format!(
                "No API key found for provider '{}'. Set the appropriate API key env var.",
                model.provider
            ));
            msg.timestamp = now_ms();
            sender.error(msg);
            return;
        }
    };

    let base_url = &model.base_url;
    let url = format!("{base_url}/v1/chat/completions");

    // Build messages
    let messages = convert_messages(context);

    // Build request body
    let max_tokens = options.max_tokens.unwrap_or(model.max_tokens.min(32000));

    // Detect compat settings from model
    let compat: Option<OpenAICompletionsCompat> = model
        .compat
        .as_ref()
        .and_then(|v| serde_json::from_value(v.clone()).ok());

    let max_tokens_field = compat
        .as_ref()
        .and_then(|c| c.max_tokens_field.as_deref())
        .unwrap_or("max_tokens");

    let mut body = json!({
        "model": model.id,
        "messages": messages,
        max_tokens_field: max_tokens,
        "stream": true,
        "stream_options": { "include_usage": true },
    });

    if let Some(temp) = options.temperature {
        body["temperature"] = json!(temp);
    }

    if let Some(ref effort) = reasoning_effort {
        body["reasoning_effort"] = json!(effort);
    }

    // Add tools
    if let Some(ref tools) = context.tools {
        let tool_defs: Vec<Value> = tools
            .iter()
            .map(|t| {
                json!({
                    "type": "function",
                    "function": {
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.parameters,
                    }
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
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Content-Type", "application/json");

    if let Some(ref headers) = options.headers {
        for (k, v) in headers {
            request = request.header(k, v);
        }
    }

    let response = match request.json(&body).send().await {
        Ok(r) => r,
        Err(e) => {
            let mut msg = AssistantMessage::default();
            msg.api = "openai-completions".to_string();
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
        msg.api = "openai-completions".to_string();
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
        api: "openai-completions".to_string(),
        provider: model.provider.clone(),
        model: model.id.clone(),
        timestamp: now_ms(),
        ..Default::default()
    };

    sender.push(AssistantMessageEvent::Start {
        partial: partial.clone(),
    });

    let mut current_text = String::new();
    let mut tool_calls: HashMap<usize, (String, String, String)> = HashMap::new(); // index -> (id, name, args_json)
    let mut text_started = false;

    let body_text = response.text().await.unwrap_or_default();
    for line in body_text.lines() {
        let line = line.trim();
        if !line.starts_with("data: ") {
            continue;
        }
        let data = &line[6..];
        if data == "[DONE]" {
            break;
        }

        let chunk: Value = match serde_json::from_str(data) {
            Ok(v) => v,
            Err(_) => continue,
        };

        // Process usage if present
        if let Some(usage) = chunk.get("usage") {
            if let Some(prompt) = usage["prompt_tokens"].as_u64() {
                partial.usage.input = prompt;
            }
            if let Some(completion) = usage["completion_tokens"].as_u64() {
                partial.usage.output = completion;
            }
            if let Some(total) = usage["total_tokens"].as_u64() {
                partial.usage.total_tokens = total;
            }
            if let Some(cached) = usage.get("prompt_tokens_details").and_then(|d| d["cached_tokens"].as_u64()) {
                partial.usage.cache_read = cached;
            }
        }

        let choices = match chunk.get("choices").and_then(|c| c.as_array()) {
            Some(c) => c,
            None => continue,
        };

        for choice in choices {
            let delta = match choice.get("delta") {
                Some(d) => d,
                None => continue,
            };

            // Check finish_reason
            if let Some(finish) = choice.get("finish_reason").and_then(|f| f.as_str()) {
                partial.stop_reason = match finish {
                    "stop" => StopReason::Stop,
                    "length" => StopReason::Length,
                    "tool_calls" => StopReason::ToolUse,
                    _ => StopReason::Stop,
                };
            }

            // Text content
            if let Some(content) = delta.get("content").and_then(|c| c.as_str()) {
                if !text_started {
                    text_started = true;
                    sender.push(AssistantMessageEvent::TextStart {
                        content_index: 0,
                        partial: partial.clone(),
                    });
                }
                current_text.push_str(content);
                sender.push(AssistantMessageEvent::TextDelta {
                    content_index: 0,
                    delta: content.to_string(),
                    partial: partial.clone(),
                });
            }

            // Reasoning content (OpenAI o-series)
            for key in &["reasoning_content", "reasoning", "reasoning_text"] {
                if let Some(reasoning) = delta.get(*key).and_then(|c| c.as_str()) {
                    sender.push(AssistantMessageEvent::ThinkingDelta {
                        content_index: 0,
                        delta: reasoning.to_string(),
                        partial: partial.clone(),
                    });
                }
            }

            // Tool calls
            if let Some(tc_array) = delta.get("tool_calls").and_then(|t| t.as_array()) {
                for tc in tc_array {
                    let idx = tc["index"].as_u64().unwrap_or(0) as usize;

                    let entry = tool_calls.entry(idx).or_insert_with(|| {
                        let id = tc
                            .get("id")
                            .and_then(|i| i.as_str())
                            .unwrap_or("")
                            .to_string();
                        let name = tc
                            .get("function")
                            .and_then(|f| f["name"].as_str())
                            .unwrap_or("")
                            .to_string();
                        sender.push(AssistantMessageEvent::ToolCallStart {
                            content_index: idx,
                            partial: partial.clone(),
                        });
                        (id, name, String::new())
                    });

                    if let Some(args) = tc
                        .get("function")
                        .and_then(|f| f["arguments"].as_str())
                    {
                        entry.2.push_str(args);
                        sender.push(AssistantMessageEvent::ToolCallDelta {
                            content_index: idx,
                            delta: args.to_string(),
                            partial: partial.clone(),
                        });
                    }
                }
            }
        }
    }

    // Finalize text block
    if text_started {
        partial.content.push(ContentBlock::Text(TextContent {
            text: current_text.clone(),
            text_signature: None,
        }));
        sender.push(AssistantMessageEvent::TextEnd {
            content_index: 0,
            content: current_text,
            partial: partial.clone(),
        });
    }

    // Finalize tool calls
    let mut indices: Vec<usize> = tool_calls.keys().cloned().collect();
    indices.sort();
    for idx in indices {
        if let Some((id, name, args_json)) = tool_calls.remove(&idx) {
            let arguments: HashMap<String, Value> =
                serde_json::from_str(&args_json).unwrap_or_default();
            let tool_call = ToolCall {
                id,
                name,
                arguments,
                thought_signature: None,
            };
            partial
                .content
                .push(ContentBlock::ToolCall(tool_call.clone()));
            sender.push(AssistantMessageEvent::ToolCallEnd {
                content_index: idx,
                tool_call,
                partial: partial.clone(),
            });
        }
    }

    // Calculate cost
    calculate_cost(model, &mut partial.usage);

    sender.finish(partial);
}

fn convert_messages(context: &Context) -> Vec<Value> {
    let mut result = Vec::new();

    // System prompt
    if let Some(ref system_prompt) = context.system_prompt {
        result.push(json!({
            "role": "system",
            "content": system_prompt,
        }));
    }

    for msg in &context.messages {
        match msg {
            Message::User(user) => {
                let content = match &user.content {
                    UserContent::Text(text) => json!(text),
                    UserContent::Blocks(blocks) => {
                        let parts: Vec<Value> = blocks
                            .iter()
                            .map(|b| match b {
                                UserContentBlock::Text(t) => {
                                    json!({"type": "text", "text": t.text})
                                }
                                UserContentBlock::Image(img) => {
                                    json!({
                                        "type": "image_url",
                                        "image_url": {
                                            "url": format!("data:{};base64,{}", img.mime_type, img.data)
                                        }
                                    })
                                }
                            })
                            .collect();
                        json!(parts)
                    }
                };
                result.push(json!({"role": "user", "content": content}));
            }
            Message::Assistant(assistant) => {
                let mut content_parts: Vec<Value> = Vec::new();
                let mut tool_calls_parts: Vec<Value> = Vec::new();

                for block in &assistant.content {
                    match block {
                        ContentBlock::Text(t) => {
                            content_parts.push(json!({"type": "text", "text": t.text}));
                        }
                        ContentBlock::Thinking(t) => {
                            // Include thinking as text for providers that need it
                            content_parts.push(json!({"type": "text", "text": format!("<thinking>{}</thinking>", t.thinking)}));
                        }
                        ContentBlock::ToolCall(tc) => {
                            tool_calls_parts.push(json!({
                                "id": tc.id,
                                "type": "function",
                                "function": {
                                    "name": tc.name,
                                    "arguments": serde_json::to_string(&tc.arguments).unwrap_or_default(),
                                }
                            }));
                        }
                        ContentBlock::Image(_) => {}
                    }
                }

                let mut msg = json!({"role": "assistant"});
                if content_parts.len() == 1 {
                    if let Some(text) = content_parts[0].get("text") {
                        msg["content"] = text.clone();
                    }
                } else if !content_parts.is_empty() {
                    msg["content"] = json!(content_parts);
                }
                if !tool_calls_parts.is_empty() {
                    msg["tool_calls"] = json!(tool_calls_parts);
                }
                result.push(msg);
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

                result.push(json!({
                    "role": "tool",
                    "tool_call_id": tr.tool_call_id,
                    "content": text,
                }));
            }
        }
    }

    result
}
