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

use super::google_shared;

pub struct GoogleProvider;

#[async_trait]
impl ApiProvider for GoogleProvider {
    fn api(&self) -> &str {
        "google-generative-ai"
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
            stream_google(&model, &context_clone, &options, None, sender).await;
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
            let thinking_config = reasoning.map(|level| match level {
                ThinkingLevel::Minimal => json!({"thinkingBudget": 1024}),
                ThinkingLevel::Low => json!({"thinkingBudget": 2048}),
                ThinkingLevel::Medium => json!({"thinkingBudget": 8192}),
                ThinkingLevel::High | ThinkingLevel::Xhigh => json!({"thinkingBudget": 16384}),
            });
            stream_google(&model, &context_clone, &base_options, thinking_config, sender).await;
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

async fn stream_google(
    model: &Model,
    context: &Context,
    options: &StreamOptions,
    thinking_config: Option<Value>,
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
            msg.api = "google-generative-ai".to_string();
            msg.provider = model.provider.clone();
            msg.model = model.id.clone();
            msg.stop_reason = StopReason::Error;
            msg.error_message = Some(format!(
                "No API key found for provider '{}'. Set GEMINI_API_KEY.",
                model.provider
            ));
            msg.timestamp = now_ms();
            sender.error(msg);
            return;
        }
    };

    let base_url = &model.base_url;
    let url = format!(
        "{base_url}/v1beta/models/{}:streamGenerateContent?key={}&alt=sse",
        model.id, api_key
    );

    // Build request body
    let contents = google_shared::convert_messages(context);

    let mut body = json!({
        "contents": contents,
    });

    if let Some(ref system_prompt) = context.system_prompt {
        body["systemInstruction"] = json!({
            "parts": [{"text": system_prompt}]
        });
    }

    if let Some(ref tools) = context.tools {
        if !tools.is_empty() {
            body["tools"] = json!(google_shared::convert_tools(tools));
        }
    }

    let mut generation_config = json!({});
    if let Some(max_tokens) = options.max_tokens {
        generation_config["maxOutputTokens"] = json!(max_tokens);
    }
    if let Some(temp) = options.temperature {
        generation_config["temperature"] = json!(temp);
    }
    if let Some(ref tc) = thinking_config {
        generation_config["thinkingConfig"] = tc.clone();
    }
    body["generationConfig"] = generation_config;

    let client = Client::new();
    let mut request = client
        .post(&url)
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
            msg.api = "google-generative-ai".to_string();
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
        msg.api = "google-generative-ai".to_string();
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
        api: "google-generative-ai".to_string(),
        provider: model.provider.clone(),
        model: model.id.clone(),
        timestamp: now_ms(),
        ..Default::default()
    };

    sender.push(AssistantMessageEvent::Start {
        partial: partial.clone(),
    });

    let mut current_text = String::new();
    let mut text_started = false;
    let mut tool_call_idx = 0;

    let body_text = response.text().await.unwrap_or_default();
    for line in body_text.lines() {
        let line = line.trim();
        if !line.starts_with("data: ") {
            continue;
        }
        let data = &line[6..];

        let chunk: Value = match serde_json::from_str(data) {
            Ok(v) => v,
            Err(_) => continue,
        };

        if let Some(candidates) = chunk.get("candidates").and_then(|c| c.as_array()) {
            for candidate in candidates {
                if let Some(finish_reason) = candidate.get("finishReason").and_then(|f| f.as_str())
                {
                    partial.stop_reason = google_shared::map_stop_reason(finish_reason);
                }

                if let Some(content) = candidate.get("content") {
                    if let Some(parts) = content.get("parts").and_then(|p| p.as_array()) {
                        for part in parts {
                            // Text content
                            if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                                let is_thought =
                                    part.get("thought").and_then(|t| t.as_bool()).unwrap_or(false);

                                if is_thought {
                                    sender.push(AssistantMessageEvent::ThinkingDelta {
                                        content_index: 0,
                                        delta: text.to_string(),
                                        partial: partial.clone(),
                                    });
                                } else {
                                    if !text_started {
                                        text_started = true;
                                        sender.push(AssistantMessageEvent::TextStart {
                                            content_index: 0,
                                            partial: partial.clone(),
                                        });
                                    }
                                    current_text.push_str(text);
                                    sender.push(AssistantMessageEvent::TextDelta {
                                        content_index: 0,
                                        delta: text.to_string(),
                                        partial: partial.clone(),
                                    });
                                }
                            }

                            // Function call
                            if let Some(fc) = part.get("functionCall") {
                                let name = fc["name"].as_str().unwrap_or("").to_string();
                                let args: HashMap<String, Value> = fc
                                    .get("args")
                                    .and_then(|a| serde_json::from_value(a.clone()).ok())
                                    .unwrap_or_default();

                                let tool_call = ToolCall {
                                    id: format!("tc_{tool_call_idx}"),
                                    name,
                                    arguments: args,
                                    thought_signature: None,
                                };
                                tool_call_idx += 1;

                                partial.stop_reason = StopReason::ToolUse;

                                sender.push(AssistantMessageEvent::ToolCallStart {
                                    content_index: tool_call_idx,
                                    partial: partial.clone(),
                                });
                                sender.push(AssistantMessageEvent::ToolCallEnd {
                                    content_index: tool_call_idx,
                                    tool_call: tool_call.clone(),
                                    partial: partial.clone(),
                                });

                                partial
                                    .content
                                    .push(ContentBlock::ToolCall(tool_call));
                            }
                        }
                    }
                }
            }
        }

        // Usage metadata
        if let Some(usage_metadata) = chunk.get("usageMetadata") {
            if let Some(prompt) = usage_metadata["promptTokenCount"].as_u64() {
                partial.usage.input = prompt;
            }
            if let Some(candidates) = usage_metadata["candidatesTokenCount"].as_u64() {
                partial.usage.output = candidates;
            }
            if let Some(total) = usage_metadata["totalTokenCount"].as_u64() {
                partial.usage.total_tokens = total;
            }
            if let Some(cached) = usage_metadata["cachedContentTokenCount"].as_u64() {
                partial.usage.cache_read = cached;
            }
        }
    }

    // Finalize text
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

    calculate_cost(model, &mut partial.usage);
    sender.finish(partial);
}
