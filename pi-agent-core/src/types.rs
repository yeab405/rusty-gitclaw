use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashMap;

use pi_ai::types::{
    AssistantMessage, AssistantMessageEvent, Message, Model, ToolResultContent,
};

// ── AgentTool ───────────────────────────────────────────────────────────

/// Result of an agent tool execution.
#[derive(Debug, Clone)]
pub struct AgentToolResult {
    pub content: Vec<ToolResultContent>,
    pub details: Option<Value>,
}

/// Callback for streaming tool execution updates.
pub type AgentToolUpdateCallback = Box<dyn Fn(AgentToolResult) + Send + Sync>;

/// Trait for agent tools (async execute).
#[async_trait]
pub trait AgentTool: Send + Sync {
    fn name(&self) -> &str;
    fn label(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters(&self) -> &Value;

    async fn execute(
        &self,
        tool_call_id: &str,
        args: HashMap<String, Value>,
        cancel: tokio_util::sync::CancellationToken,
        on_update: Option<AgentToolUpdateCallback>,
    ) -> Result<AgentToolResult, String>;
}

/// Boxed agent tool for type erasure.
pub type BoxedAgentTool = Box<dyn AgentTool>;

// ── ThinkingLevel ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentThinkingLevel {
    Off,
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
}

impl AgentThinkingLevel {
    pub fn to_pi_ai(&self) -> Option<pi_ai::types::ThinkingLevel> {
        match self {
            Self::Off => None,
            Self::Minimal => Some(pi_ai::types::ThinkingLevel::Minimal),
            Self::Low => Some(pi_ai::types::ThinkingLevel::Low),
            Self::Medium => Some(pi_ai::types::ThinkingLevel::Medium),
            Self::High => Some(pi_ai::types::ThinkingLevel::High),
            Self::Xhigh => Some(pi_ai::types::ThinkingLevel::Xhigh),
        }
    }
}

// ── AgentState ──────────────────────────────────────────────────────────

pub struct AgentState {
    pub system_prompt: Option<String>,
    pub model: Model,
    pub thinking_level: AgentThinkingLevel,
    pub tools: Vec<BoxedAgentTool>,
    pub messages: Vec<AgentMessage>,
    pub is_streaming: bool,
    pub stream_message: Option<AssistantMessage>,
    pub pending_tool_calls: Vec<pi_ai::types::ToolCall>,
    pub error: Option<String>,
}

// ── AgentMessage ────────────────────────────────────────────────────────

/// AgentMessage is the union of LLM Message + any custom messages.
/// For simplicity in the Rust port, we use the pi_ai Message type directly.
pub type AgentMessage = Message;

// ── AgentEvent ──────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum AgentEvent {
    AgentStart,
    AgentEnd,
    TurnStart {
        turn: usize,
    },
    TurnEnd {
        turn: usize,
    },
    MessageStart,
    MessageUpdate {
        assistant_message_event: AssistantMessageEvent,
    },
    MessageEnd {
        message: AgentMessage,
    },
    ToolExecutionStart {
        tool_call_id: String,
        tool_name: String,
        args: Option<HashMap<String, Value>>,
    },
    ToolExecutionUpdate {
        tool_call_id: String,
        tool_name: String,
        result: AgentToolResult,
    },
    ToolExecutionEnd {
        tool_call_id: String,
        tool_name: String,
        result: Option<AgentToolResult>,
        is_error: bool,
    },
}
