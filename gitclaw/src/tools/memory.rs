use async_trait::async_trait;
use pi_agent_core::types::{AgentToolResult, AgentToolUpdateCallback};
use pi_ai::types::{TextContent, ToolResultContent};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command as StdCommand;
use tokio_util::sync::CancellationToken;

use super::shared::DEFAULT_MEMORY_PATH;

pub struct MemoryTool {
    cwd: PathBuf,
}

impl MemoryTool {
    pub fn new(cwd: PathBuf) -> Self {
        Self { cwd }
    }
}

fn text_result(text: String) -> AgentToolResult {
    AgentToolResult {
        content: vec![ToolResultContent::Text(TextContent {
            text,
            text_signature: None,
        })],
        details: None,
    }
}

#[async_trait]
impl pi_agent_core::types::AgentTool for MemoryTool {
    fn name(&self) -> &str { "memory" }
    fn label(&self) -> &str { "memory" }
    fn description(&self) -> &str {
        "Git-backed memory. Use 'load' to read current memory, 'save' to update memory and commit to git. Each save creates a git commit, giving you full history of what you've remembered."
    }
    fn parameters(&self) -> &Value {
        static SCHEMA: once_cell::sync::Lazy<Value> = once_cell::sync::Lazy::new(|| {
            json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["load", "save"],
                        "description": "Whether to load or save memory"
                    },
                    "content": {
                        "type": "string",
                        "description": "Memory content to save (required for save)"
                    },
                    "message": {
                        "type": "string",
                        "description": "Commit message describing why this memory changed (required for save)"
                    }
                },
                "required": ["action"]
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

        let action = args.get("action")
            .and_then(|v| v.as_str())
            .ok_or("Missing 'action' argument")?;

        let memory_path = DEFAULT_MEMORY_PATH;
        let memory_file = self.cwd.join(memory_path);

        if action == "load" {
            match tokio::fs::read_to_string(&memory_file).await {
                Ok(text) => {
                    let trimmed = text.trim();
                    if trimmed.is_empty() || trimmed == "# Memory" {
                        return Ok(text_result("No memories yet.".to_string()));
                    }
                    Ok(text_result(trimmed.to_string()))
                }
                Err(_) => Ok(text_result("No memories yet.".to_string())),
            }
        } else {
            // save
            let content = args.get("content")
                .and_then(|v| v.as_str())
                .ok_or("content is required for save action")?;
            let commit_msg = args.get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("Update memory");

            // Create directories and write
            if let Some(parent) = memory_file.parent() {
                tokio::fs::create_dir_all(parent).await
                    .map_err(|e| format!("Failed to create memory directory: {e}"))?;
            }
            tokio::fs::write(&memory_file, content).await
                .map_err(|e| format!("Failed to write memory: {e}"))?;

            // Git commit
            let escaped_msg = commit_msg.replace('"', "\\\"");
            let git_cmd = format!("git add \"{memory_path}\" && git commit -m \"{escaped_msg}\"");
            match StdCommand::new("sh")
                .arg("-c")
                .arg(&git_cmd)
                .current_dir(&self.cwd)
                .output()
            {
                Ok(output) if output.status.success() => {
                    Ok(text_result(format!("Memory saved and committed: \"{commit_msg}\"")))
                }
                Ok(output) => {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    Ok(text_result(format!(
                        "Memory saved to {memory_path} but git commit failed: {}. The file was still written.",
                        stderr.trim()
                    )))
                }
                Err(_) => {
                    Ok(text_result(format!(
                        "Memory saved to {memory_path} but git commit failed. The file was still written."
                    )))
                }
            }
        }
    }
}
