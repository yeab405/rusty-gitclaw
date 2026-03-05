use std::collections::HashMap;
use std::path::PathBuf;
use tokio::sync::mpsc;

use pi_agent_core::agent::{Agent, AgentOptions};
use pi_agent_core::types::{AgentEvent, BoxedAgentTool};
use pi_ai::types::AssistantMessageEvent;

use crate::hooks::{load_hooks_config, run_hooks};
use crate::loader::{load_agent, AgentManifest};
use crate::sdk_types::*;
use crate::tool_loader::load_declarative_tools;
use crate::tools::{create_builtin_tools, BuiltinToolsConfig};

/// The `Query` struct: an async receiver of `GCMessage` values with helper methods.
pub struct Query {
    rx: mpsc::UnboundedReceiver<GCMessage>,
    collected: Vec<GCMessage>,
    session_id: String,
    manifest: Option<AgentManifest>,
}

impl Query {
    /// Receive the next message (async).
    pub async fn next(&mut self) -> Option<GCMessage> {
        let msg = self.rx.recv().await?;
        self.collected.push(msg.clone());
        Some(msg)
    }

    /// Abort the query (placeholder — full cancel requires CancellationToken propagation).
    pub fn abort(&self) {
        // In a full implementation, this would cancel via CancellationToken.
    }

    /// Get the session ID.
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Get the agent manifest (available after agent is loaded).
    pub fn manifest(&self) -> Option<&AgentManifest> {
        self.manifest.as_ref()
    }

    /// Get all collected messages so far.
    pub fn messages(&self) -> &[GCMessage] {
        &self.collected
    }
}

/// Extract text and thinking from an AssistantMessage.
fn extract_content(msg: &pi_ai::types::AssistantMessage) -> (String, String) {
    let mut text = String::new();
    let mut thinking = String::new();
    for block in &msg.content {
        match block {
            pi_ai::types::ContentBlock::Text(tc) => text.push_str(&tc.text),
            pi_ai::types::ContentBlock::Thinking(tc) => thinking.push_str(&tc.thinking),
            _ => {}
        }
    }
    (text, thinking)
}

/// Start a query against a gitclaw agent. Returns a `Query` that yields `GCMessage` events.
pub fn query(options: QueryOptions) -> Query {
    let (tx, rx) = mpsc::unbounded_channel();
    let session_id = options.session_id.clone().unwrap_or_default();

    let query = Query {
        rx,
        collected: Vec::new(),
        session_id: session_id.clone(),
        manifest: None,
    };

    // Spawn the async initialization + run
    tokio::spawn(async move {
        if let Err(e) = run_query(options, tx.clone()).await {
            let _ = tx.send(GCMessage::System(GCSystemMessage {
                subtype: "error".to_string(),
                content: e,
                metadata: None,
            }));
        }
        // Dropping tx closes the channel, signaling end.
    });

    query
}

async fn run_query(
    options: QueryOptions,
    tx: mpsc::UnboundedSender<GCMessage>,
) -> Result<(), String> {
    // Validate mutually exclusive options
    if options.repo.is_some() {
        return Err("repo and sandbox options are mutually exclusive with each other".to_string());
    }

    let dir = options
        .dir
        .as_deref()
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

    // 1. Load agent
    let loaded = load_agent(&dir, options.model.as_deref(), options.env.as_deref())
        .await
        .map_err(|e| format!("Failed to load agent: {e}"))?;

    let session_id = if options.session_id.as_deref().unwrap_or("").is_empty() {
        loaded.session_id.clone()
    } else {
        options.session_id.clone().unwrap_or_default()
    };

    // 2. Apply system prompt overrides
    let mut system_prompt = loaded.system_prompt.clone();
    if let Some(ref sp) = options.system_prompt {
        system_prompt = sp.clone();
    }
    if let Some(ref suffix) = options.system_prompt_suffix {
        system_prompt = format!("{system_prompt}\n\n{suffix}");
    }

    // 3. Build tools
    let mut tools: Vec<BoxedAgentTool> = if !options.replace_builtin_tools {
        create_builtin_tools(&BuiltinToolsConfig {
            dir: dir.clone(),
            timeout: loaded.manifest.runtime.timeout,
        })
    } else {
        Vec::new()
    };

    // Declarative tools from tools/*.yaml
    let declarative_tools = load_declarative_tools(&dir).await;
    tools.extend(declarative_tools);

    // Filter by allowlist/denylist
    if let Some(ref allowed) = options.allowed_tools {
        let allowed_set: std::collections::HashSet<&str> =
            allowed.iter().map(|s| s.as_str()).collect();
        tools.retain(|t| allowed_set.contains(t.name()));
    }
    if let Some(ref denied) = options.disallowed_tools {
        let denied_set: std::collections::HashSet<&str> =
            denied.iter().map(|s| s.as_str()).collect();
        tools.retain(|t| !denied_set.contains(t.name()));
    }

    // 4. Load hooks config (hook wrapping is handled at the CLI level)
    let hooks_config = load_hooks_config(&loaded.agent_dir).await;

    // 5. Run on_session_start hooks
    if let Some(ref hc) = hooks_config {
        if let Some(ref hooks) = hc.hooks.on_session_start {
            let result = run_hooks(
                hooks,
                &loaded.agent_dir,
                &serde_json::json!({
                    "event": "on_session_start",
                    "session_id": session_id,
                    "agent": loaded.manifest.name,
                }),
            )
            .await;
            if result.action == "block" {
                let _ = tx.send(GCMessage::System(GCSystemMessage {
                    subtype: "hook_blocked".to_string(),
                    content: format!(
                        "Session blocked by hook: {}",
                        result.reason.unwrap_or_else(|| "no reason given".to_string())
                    ),
                    metadata: None,
                }));
                return Ok(());
            }
        }
    }

    // 6. Build model options from constraints
    let constraints = options.constraints.or_else(|| {
        loaded
            .manifest
            .model
            .constraints
            .as_ref()
            .and_then(|c| serde_yaml::from_value(c.clone()).ok())
    });

    let temperature = constraints.as_ref().and_then(|c| c.temperature);
    let max_tokens = constraints.as_ref().and_then(|c| c.max_tokens);

    // 7. Create Agent
    let agent = Agent::new(AgentOptions {
        system_prompt: Some(system_prompt),
        model: loaded.model.clone(),
        tools,
        thinking_level: None,
        temperature,
        max_tokens,
    });

    // 8. Subscribe to events and map to GCMessage
    let mut event_rx = agent.subscribe();
    let tx_events = tx.clone();
    let manifest_name = loaded.manifest.name.clone();
    let sid = session_id.clone();

    tokio::spawn(async move {
        let mut acc_text = String::new();
        let mut acc_thinking = String::new();

        while let Ok(event) = event_rx.recv().await {
            match event {
                AgentEvent::AgentStart => {
                    let _ = tx_events.send(GCMessage::System(GCSystemMessage {
                        subtype: "session_start".to_string(),
                        content: format!("Agent {} started", manifest_name),
                        metadata: Some({
                            let mut m = HashMap::new();
                            m.insert(
                                "sessionId".to_string(),
                                serde_json::Value::String(sid.clone()),
                            );
                            m
                        }),
                    }));
                }
                AgentEvent::MessageUpdate {
                    assistant_message_event,
                } => match assistant_message_event {
                    AssistantMessageEvent::TextDelta { delta, .. } => {
                        acc_text.push_str(&delta);
                        let _ = tx_events.send(GCMessage::Delta(GCStreamDelta {
                            delta_type: "text".to_string(),
                            content: delta,
                        }));
                    }
                    AssistantMessageEvent::ThinkingDelta { delta, .. } => {
                        acc_thinking.push_str(&delta);
                        let _ = tx_events.send(GCMessage::Delta(GCStreamDelta {
                            delta_type: "thinking".to_string(),
                            content: delta,
                        }));
                    }
                    _ => {}
                },
                AgentEvent::MessageEnd { message } => {
                    if let pi_ai::types::Message::Assistant(ref msg) = message {
                        let (text, thinking) = extract_content(msg);
                        let content = if text.is_empty() {
                            std::mem::take(&mut acc_text)
                        } else {
                            text
                        };
                        let think = if thinking.is_empty() {
                            std::mem::take(&mut acc_thinking)
                        } else {
                            thinking
                        };

                        let assistant_msg = GCAssistantMessage {
                            content,
                            thinking: if think.is_empty() { None } else { Some(think) },
                            model: msg.model.clone(),
                            provider: msg.provider.clone(),
                            stop_reason: format!("{:?}", msg.stop_reason).to_lowercase(),
                            error_message: msg.error_message.clone(),
                            usage: Some(GCUsage {
                                input_tokens: msg.usage.input,
                                output_tokens: msg.usage.output,
                                cache_read_tokens: msg.usage.cache_read,
                                cache_write_tokens: msg.usage.cache_write,
                                total_tokens: msg.usage.total_tokens,
                                cost_usd: msg.usage.cost.total,
                            }),
                        };
                        let _ = tx_events.send(GCMessage::Assistant(assistant_msg));

                        acc_text.clear();
                        acc_thinking.clear();
                    }
                }
                AgentEvent::ToolExecutionStart {
                    tool_call_id,
                    tool_name,
                    args,
                } => {
                    let _ = tx_events.send(GCMessage::ToolUse(GCToolUseMessage {
                        tool_call_id,
                        tool_name,
                        args: args.unwrap_or_default(),
                    }));
                }
                AgentEvent::ToolExecutionEnd {
                    tool_call_id,
                    tool_name,
                    result,
                    is_error,
                } => {
                    let content = result
                        .and_then(|r| {
                            r.content
                                .first()
                                .and_then(|c| {
                                    if let pi_ai::types::ToolResultContent::Text(t) = c {
                                        Some(t.text.clone())
                                    } else {
                                        None
                                    }
                                })
                        })
                        .unwrap_or_default();
                    let _ = tx_events.send(GCMessage::ToolResult(GCToolResultMessage {
                        tool_call_id,
                        tool_name,
                        content,
                        is_error,
                    }));
                }
                AgentEvent::AgentEnd => {
                    // Channel will close when tx is dropped
                }
                _ => {}
            }
        }
    });

    // 9. Send prompt
    agent
        .prompt(&options.prompt)
        .await
        .map_err(|e| format!("Agent error: {e}"))?;

    Ok(())
}

/// Helper to create a `GCToolDefinition`.
pub fn tool(
    name: impl Into<String>,
    description: impl Into<String>,
    input_schema: serde_json::Value,
    handler: impl Fn(serde_json::Value) -> futures::future::BoxFuture<'static, Result<String, String>>
        + Send
        + Sync
        + 'static,
) -> GCToolDefinition {
    GCToolDefinition {
        name: name.into(),
        description: description.into(),
        input_schema,
        handler: Box::new(handler),
    }
}
