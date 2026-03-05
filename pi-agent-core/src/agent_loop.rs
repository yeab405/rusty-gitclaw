use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use pi_ai::types::*;
use pi_ai::validation::validate_tool_call;

use crate::types::*;

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Configuration for the agent loop.
pub struct AgentLoopConfig {
    pub model: Model,
    pub system_prompt: Option<String>,
    pub tools: Vec<Tool>,
    pub agent_tools: Vec<BoxedAgentTool>,
    pub thinking_level: AgentThinkingLevel,
    pub temperature: Option<f64>,
    pub max_tokens: Option<u32>,
}

/// Run the core agent loop.
///
/// Takes initial messages, streams LLM response, executes tool calls, repeats.
/// Emits AgentEvents via the provided sender.
pub async fn run_loop(
    config: AgentLoopConfig,
    initial_messages: Vec<AgentMessage>,
    event_tx: mpsc::UnboundedSender<AgentEvent>,
    cancel: CancellationToken,
) -> Vec<AgentMessage> {
    let mut messages = initial_messages;
    let mut turn = 0;

    let _ = event_tx.send(AgentEvent::AgentStart);

    loop {
        if cancel.is_cancelled() {
            break;
        }

        turn += 1;
        let _ = event_tx.send(AgentEvent::TurnStart { turn });

        // Stream assistant response
        let assistant_msg = match stream_assistant_response(
            &config,
            &messages,
            &event_tx,
            &cancel,
        )
        .await
        {
            Some(msg) => msg,
            None => break, // Cancelled or error
        };

        // Add assistant message to history
        messages.push(Message::Assistant(assistant_msg.clone()));
        let _ = event_tx.send(AgentEvent::MessageEnd {
            message: Message::Assistant(assistant_msg.clone()),
        });

        // Check if we should execute tool calls
        let tool_calls: Vec<ToolCall> = assistant_msg
            .content
            .iter()
            .filter_map(|b| match b {
                ContentBlock::ToolCall(tc) => Some(tc.clone()),
                _ => None,
            })
            .collect();

        if tool_calls.is_empty() || assistant_msg.stop_reason != StopReason::ToolUse {
            let _ = event_tx.send(AgentEvent::TurnEnd { turn });
            break;
        }

        // Execute tool calls
        let tool_results =
            execute_tool_calls(&config, &tool_calls, &event_tx, &cancel).await;

        // Add tool results to messages
        for result in tool_results {
            messages.push(Message::ToolResult(result));
        }

        let _ = event_tx.send(AgentEvent::TurnEnd { turn });

        // Loop back for another LLM call with tool results
    }

    let _ = event_tx.send(AgentEvent::AgentEnd);
    messages
}

async fn stream_assistant_response(
    config: &AgentLoopConfig,
    messages: &[AgentMessage],
    event_tx: &mpsc::UnboundedSender<AgentEvent>,
    cancel: &CancellationToken,
) -> Option<AssistantMessage> {
    let _ = event_tx.send(AgentEvent::MessageStart);

    // Build context
    let context = Context {
        system_prompt: config.system_prompt.clone(),
        messages: messages.to_vec(),
        tools: if config.tools.is_empty() {
            None
        } else {
            Some(config.tools.clone())
        },
    };

    // Build stream options
    let simple_options = SimpleStreamOptions {
        base: StreamOptions {
            temperature: config.temperature,
            max_tokens: config.max_tokens,
            ..Default::default()
        },
        reasoning: config.thinking_level.to_pi_ai(),
        thinking_budgets: None,
    };

    // Stream
    let mut stream = match pi_ai::stream_simple(&config.model, &context, &simple_options) {
        Ok(s) => s,
        Err(e) => {
            let mut error_msg = AssistantMessage::default();
            error_msg.stop_reason = StopReason::Error;
            error_msg.error_message = Some(format!("Failed to start stream: {e}"));
            error_msg.model = config.model.id.clone();
            error_msg.provider = config.model.provider.clone();
            error_msg.api = config.model.api.clone();
            error_msg.timestamp = now_ms();
            let _ = event_tx.send(AgentEvent::MessageUpdate {
                assistant_message_event: AssistantMessageEvent::Error {
                    reason: StopReason::Error,
                    error: error_msg.clone(),
                },
            });
            return Some(error_msg);
        }
    };

    let mut last_msg: Option<AssistantMessage> = None;

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                let mut aborted = last_msg.unwrap_or_default();
                aborted.stop_reason = StopReason::Aborted;
                aborted.timestamp = now_ms();
                return Some(aborted);
            }
            event = stream.recv() => {
                match event {
                    Some(evt) => {
                        // Extract message from terminal events
                        match &evt {
                            AssistantMessageEvent::Done { message, .. } => {
                                last_msg = Some(message.clone());
                            }
                            AssistantMessageEvent::Error { error, .. } => {
                                last_msg = Some(error.clone());
                            }
                            _ => {}
                        }

                        let is_terminal = evt.is_terminal();
                        let _ = event_tx.send(AgentEvent::MessageUpdate {
                            assistant_message_event: evt,
                        });

                        if is_terminal {
                            return last_msg;
                        }
                    }
                    None => {
                        // Stream ended without terminal event
                        return last_msg;
                    }
                }
            }
        }
    }
}

async fn execute_tool_calls(
    config: &AgentLoopConfig,
    tool_calls: &[ToolCall],
    event_tx: &mpsc::UnboundedSender<AgentEvent>,
    cancel: &CancellationToken,
) -> Vec<ToolResultMessage> {
    let mut results = Vec::new();

    for tc in tool_calls {
        if cancel.is_cancelled() {
            results.push(ToolResultMessage {
                tool_call_id: tc.id.clone(),
                tool_name: tc.name.clone(),
                content: vec![ToolResultContent::Text(TextContent {
                    text: "Operation aborted".to_string(),
                    text_signature: None,
                })],
                details: None,
                is_error: true,
                timestamp: now_ms(),
            });
            continue;
        }

        let _ = event_tx.send(AgentEvent::ToolExecutionStart {
            tool_call_id: tc.id.clone(),
            tool_name: tc.name.clone(),
            args: Some(tc.arguments.clone()),
        });

        // Find the tool
        let agent_tool = config.agent_tools.iter().find(|t| t.name() == tc.name);

        let (result, is_error) = match agent_tool {
            Some(tool) => {
                // Validate arguments
                let validated_args = match validate_tool_call(&config.tools, tc) {
                    Ok(val) => {
                        serde_json::from_value::<HashMap<String, serde_json::Value>>(val)
                            .unwrap_or_else(|_| tc.arguments.clone())
                    }
                    Err(e) => {
                        let err_result = AgentToolResult {
                            content: vec![ToolResultContent::Text(TextContent {
                                text: format!("Argument validation failed: {e}"),
                                text_signature: None,
                            })],
                            details: None,
                        };
                        let _ = event_tx.send(AgentEvent::ToolExecutionEnd {
                            tool_call_id: tc.id.clone(),
                            tool_name: tc.name.clone(),
                            result: Some(err_result.clone()),
                            is_error: true,
                        });
                        results.push(ToolResultMessage {
                            tool_call_id: tc.id.clone(),
                            tool_name: tc.name.clone(),
                            content: err_result.content,
                            details: err_result.details,
                            is_error: true,
                            timestamp: now_ms(),
                        });
                        continue;
                    }
                };

                // Execute
                match tool
                    .execute(&tc.id, validated_args, cancel.clone(), None)
                    .await
                {
                    Ok(result) => (result, false),
                    Err(e) => {
                        let err_result = AgentToolResult {
                            content: vec![ToolResultContent::Text(TextContent {
                                text: e,
                                text_signature: None,
                            })],
                            details: None,
                        };
                        (err_result, true)
                    }
                }
            }
            None => {
                let err_result = AgentToolResult {
                    content: vec![ToolResultContent::Text(TextContent {
                        text: format!("Tool '{}' not found", tc.name),
                        text_signature: None,
                    })],
                    details: None,
                };
                (err_result, true)
            }
        };

        let _ = event_tx.send(AgentEvent::ToolExecutionEnd {
            tool_call_id: tc.id.clone(),
            tool_name: tc.name.clone(),
            result: Some(result.clone()),
            is_error,
        });

        results.push(ToolResultMessage {
            tool_call_id: tc.id.clone(),
            tool_name: tc.name.clone(),
            content: result.content,
            details: result.details,
            is_error,
            timestamp: now_ms(),
        });
    }

    results
}
