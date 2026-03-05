use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use crate::loader::AgentManifest;

// Message types

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum GCMessage {
    Assistant(GCAssistantMessage),
    User(GCUserMessage),
    ToolUse(GCToolUseMessage),
    ToolResult(GCToolResultMessage),
    System(GCSystemMessage),
    Delta(GCStreamDelta),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GCAssistantMessage {
    pub content: String,
    pub thinking: Option<String>,
    pub model: String,
    pub provider: String,
    pub stop_reason: String,
    pub error_message: Option<String>,
    pub usage: Option<GCUsage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GCUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub total_tokens: u64,
    pub cost_usd: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GCUserMessage {
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GCToolUseMessage {
    pub tool_call_id: String,
    pub tool_name: String,
    pub args: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GCToolResultMessage {
    pub tool_call_id: String,
    pub tool_name: String,
    pub content: String,
    pub is_error: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GCSystemMessage {
    pub subtype: String,
    pub content: String,
    pub metadata: Option<HashMap<String, serde_json::Value>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GCStreamDelta {
    pub delta_type: String,
    pub content: String,
}

// Hook types

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GCHookContext {
    pub session_id: String,
    pub agent_name: String,
    pub event: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GCPreToolUseContext {
    pub session_id: String,
    pub agent_name: String,
    pub event: String,
    pub tool_name: String,
    pub args: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GCHookResult {
    pub action: String,
    pub reason: Option<String>,
    pub args: Option<HashMap<String, serde_json::Value>>,
}

// Tool definition

pub struct GCToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
    pub handler: Box<dyn Fn(serde_json::Value) -> futures::future::BoxFuture<'static, Result<String, String>> + Send + Sync>,
}

// Local repo options

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalRepoOptions {
    pub url: String,
    pub token: String,
    pub dir: Option<String>,
    pub session: Option<String>,
}

// Sandbox options

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxOptions {
    pub provider: String,
    pub template: Option<String>,
    pub timeout: Option<u64>,
    pub repository: Option<String>,
    pub token: Option<String>,
    pub session: Option<String>,
    pub auto_commit: Option<bool>,
    pub envs: Option<HashMap<String, String>>,
}

// Query options

pub struct QueryOptions {
    pub prompt: String,
    pub dir: Option<String>,
    pub model: Option<String>,
    pub env: Option<String>,
    pub system_prompt: Option<String>,
    pub system_prompt_suffix: Option<String>,
    pub tools: Option<Vec<GCToolDefinition>>,
    pub replace_builtin_tools: bool,
    pub allowed_tools: Option<Vec<String>>,
    pub disallowed_tools: Option<Vec<String>>,
    pub repo: Option<LocalRepoOptions>,
    pub max_turns: Option<usize>,
    pub session_id: Option<String>,
    pub constraints: Option<QueryConstraints>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryConstraints {
    pub temperature: Option<f64>,
    pub max_tokens: Option<u32>,
    pub top_p: Option<f64>,
    pub top_k: Option<u32>,
}
