use async_trait::async_trait;
use pi_agent_core::types::{AgentToolResult, AgentToolUpdateCallback};
use pi_ai::types::{TextContent, ToolResultContent};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

use super::shared::{truncate_output, DEFAULT_TIMEOUT, MAX_OUTPUT};

pub struct CliTool {
    cwd: PathBuf,
    default_timeout: u64,
}

impl CliTool {
    pub fn new(cwd: PathBuf, timeout: Option<u64>) -> Self {
        Self {
            cwd,
            default_timeout: timeout.unwrap_or(DEFAULT_TIMEOUT),
        }
    }
}

#[async_trait]
impl pi_agent_core::types::AgentTool for CliTool {
    fn name(&self) -> &str { "cli" }
    fn label(&self) -> &str { "cli" }
    fn description(&self) -> &str {
        "Execute a shell command. Returns stdout and stderr combined. Output is truncated if it exceeds ~100KB. Default timeout is 120 seconds."
    }
    fn parameters(&self) -> &Value {
        static SCHEMA: once_cell::sync::Lazy<Value> = once_cell::sync::Lazy::new(|| {
            json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "Shell command to execute" },
                    "timeout": { "type": "number", "description": "Timeout in seconds (default: 120)" }
                },
                "required": ["command"]
            })
        });
        &SCHEMA
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        args: HashMap<String, Value>,
        cancel: CancellationToken,
        _on_update: Option<AgentToolUpdateCallback>,
    ) -> Result<AgentToolResult, String> {
        if cancel.is_cancelled() {
            return Err("Operation aborted".to_string());
        }

        let command = args.get("command")
            .and_then(|v| v.as_str())
            .ok_or("Missing 'command' argument")?;

        let timeout_secs = args.get("timeout")
            .and_then(|v| v.as_u64())
            .unwrap_or(self.default_timeout);

        let output = tokio::time::timeout(
            std::time::Duration::from_secs(timeout_secs),
            Command::new("sh")
                .arg("-c")
                .arg(command)
                .current_dir(&self.cwd)
                .output(),
        )
        .await
        .map_err(|_| format!("Command timed out after {timeout_secs} seconds"))?
        .map_err(|e| format!("Failed to execute command: {e}"))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let mut text = format!("{stdout}{stderr}");

        if text.is_empty() {
            text = "(no output)".to_string();
        } else {
            text = truncate_output(&text);
        }

        let code = output.status.code().unwrap_or(-1);
        if code != 0 {
            text.push_str(&format!("\n\nExit code: {code}"));
            return Err(text);
        }

        Ok(AgentToolResult {
            content: vec![ToolResultContent::Text(TextContent {
                text,
                text_signature: None,
            })],
            details: Some(json!({"exitCode": code})),
        })
    }
}
