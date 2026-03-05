use std::sync::{Arc, Mutex};
use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;

use pi_ai::types::{Message, Model, Tool, UserContent, UserMessage};

use crate::agent_loop::{run_loop, AgentLoopConfig};
use crate::error::AgentError;
use crate::types::*;

use std::time::{SystemTime, UNIX_EPOCH};

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Agent options for construction.
pub struct AgentOptions {
    pub system_prompt: Option<String>,
    pub model: Model,
    pub tools: Vec<BoxedAgentTool>,
    pub thinking_level: Option<AgentThinkingLevel>,
    pub temperature: Option<f64>,
    pub max_tokens: Option<u32>,
}

/// The Agent struct: high-level API for running agent loops.
pub struct Agent {
    system_prompt: Option<String>,
    model: Model,
    tools: Vec<BoxedAgentTool>,
    thinking_level: AgentThinkingLevel,
    temperature: Option<f64>,
    max_tokens: Option<u32>,
    messages: Arc<Mutex<Vec<AgentMessage>>>,
    is_streaming: Arc<Mutex<bool>>,
    cancel: CancellationToken,
    event_tx: broadcast::Sender<AgentEvent>,
}

impl Agent {
    pub fn new(options: AgentOptions) -> Self {
        let (event_tx, _) = broadcast::channel(256);
        Self {
            system_prompt: options.system_prompt,
            model: options.model,
            tools: options.tools,
            thinking_level: options.thinking_level.unwrap_or(AgentThinkingLevel::Off),
            temperature: options.temperature,
            max_tokens: options.max_tokens,
            messages: Arc::new(Mutex::new(Vec::new())),
            is_streaming: Arc::new(Mutex::new(false)),
            cancel: CancellationToken::new(),
            event_tx,
        }
    }

    /// Subscribe to agent events. Returns a broadcast receiver.
    pub fn subscribe(&self) -> broadcast::Receiver<AgentEvent> {
        self.event_tx.subscribe()
    }

    /// Subscribe with a callback. Returns an unsubscribe closure.
    pub fn subscribe_fn<F>(&self, callback: F) -> tokio::task::JoinHandle<()>
    where
        F: Fn(AgentEvent) + Send + 'static,
    {
        let mut rx = self.event_tx.subscribe();
        tokio::spawn(async move {
            while let Ok(event) = rx.recv().await {
                callback(event);
            }
        })
    }

    /// Send a prompt to the agent.
    pub async fn prompt(&self, text: &str) -> Result<(), AgentError> {
        {
            let is_streaming = self.is_streaming.lock().unwrap();
            if *is_streaming {
                return Err(AgentError::AlreadyStreaming);
            }
        }

        // Create user message
        let user_msg = Message::User(UserMessage {
            content: UserContent::Text(text.to_string()),
            timestamp: now_ms(),
        });

        // Add to messages
        {
            let mut msgs = self.messages.lock().unwrap();
            msgs.push(user_msg);
        }

        self.run_agent_loop().await
    }

    /// Abort the current streaming operation.
    pub fn abort(&self) {
        self.cancel.cancel();
    }

    /// Get current state snapshot.
    pub fn is_streaming(&self) -> bool {
        *self.is_streaming.lock().unwrap()
    }

    pub fn messages(&self) -> Vec<AgentMessage> {
        self.messages.lock().unwrap().clone()
    }

    pub fn model(&self) -> &Model {
        &self.model
    }

    pub fn set_model(&mut self, model: Model) {
        self.model = model;
    }

    pub fn set_system_prompt(&mut self, prompt: Option<String>) {
        self.system_prompt = prompt;
    }

    pub fn set_thinking_level(&mut self, level: AgentThinkingLevel) {
        self.thinking_level = level;
    }

    async fn run_agent_loop(&self) -> Result<(), AgentError> {
        {
            let mut is_streaming = self.is_streaming.lock().unwrap();
            *is_streaming = true;
        }

        let (loop_event_tx, mut loop_event_rx) = mpsc::unbounded_channel();
        let cancel = self.cancel.clone();

        // Build tool schemas for the loop
        let tool_schemas: Vec<Tool> = self
            .tools
            .iter()
            .map(|t| Tool {
                name: t.name().to_string(),
                description: t.description().to_string(),
                parameters: t.parameters().clone(),
            })
            .collect();

        let config = AgentLoopConfig {
            model: self.model.clone(),
            system_prompt: self.system_prompt.clone(),
            tools: tool_schemas,
            agent_tools: Vec::new(), // TODO: pass tools by reference
            thinking_level: self.thinking_level,
            temperature: self.temperature,
            max_tokens: self.max_tokens,
        };

        let initial_messages = self.messages.lock().unwrap().clone();
        let messages_ref = Arc::clone(&self.messages);
        let is_streaming_ref = Arc::clone(&self.is_streaming);
        let event_broadcast = self.event_tx.clone();

        // Spawn the agent loop
        let loop_handle = tokio::spawn(async move {
            let result = run_loop(config, initial_messages, loop_event_tx, cancel).await;
            // Update messages
            let mut msgs = messages_ref.lock().unwrap();
            *msgs = result;
            // Mark streaming as done
            let mut is_streaming = is_streaming_ref.lock().unwrap();
            *is_streaming = false;
        });

        // Forward events from the loop to broadcast subscribers
        tokio::spawn(async move {
            while let Some(event) = loop_event_rx.recv().await {
                let _ = event_broadcast.send(event);
            }
        });

        loop_handle.await.map_err(|e| AgentError::Other(e.to_string()))
    }
}
