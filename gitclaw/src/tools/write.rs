use async_trait::async_trait;
use pi_agent_core::types::{AgentToolResult, AgentToolUpdateCallback};
use pi_ai::types::{TextContent, ToolResultContent};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use tokio_util::sync::CancellationToken;

pub struct WriteTool {
    cwd: PathBuf,
}

impl WriteTool {
    pub fn new(cwd: PathBuf) -> Self {
        Self { cwd }
    }

    fn resolve_path(&self, path: &str) -> PathBuf {
        if path.starts_with('/') {
            PathBuf::from(path)
        } else {
            self.cwd.join(path)
        }
    }
}

#[async_trait]
impl pi_agent_core::types::AgentTool for WriteTool {
    fn name(&self) -> &str { "write" }
    fn label(&self) -> &str { "write" }
    fn description(&self) -> &str {
        "Write content to a file. Creates the file if it doesn't exist, overwrites if it does. Parent directories are created automatically."
    }
    fn parameters(&self) -> &Value {
        static SCHEMA: once_cell::sync::Lazy<Value> = once_cell::sync::Lazy::new(|| {
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Path to the file to write (relative or absolute)" },
                    "content": { "type": "string", "description": "Content to write to the file" },
                    "createDirs": { "type": "boolean", "description": "Create parent directories if needed (default: true)" }
                },
                "required": ["path", "content"]
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

        let path = args.get("path")
            .and_then(|v| v.as_str())
            .ok_or("Missing 'path' argument")?;
        let content = args.get("content")
            .and_then(|v| v.as_str())
            .ok_or("Missing 'content' argument")?;
        let create_dirs = args.get("createDirs")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        let abs_path = self.resolve_path(path);

        if create_dirs {
            if let Some(parent) = abs_path.parent() {
                tokio::fs::create_dir_all(parent).await
                    .map_err(|e| format!("Failed to create directories: {e}"))?;
            }
        }

        tokio::fs::write(&abs_path, content).await
            .map_err(|e| format!("Failed to write {path}: {e}"))?;

        let bytes = content.len();
        Ok(AgentToolResult {
            content: vec![ToolResultContent::Text(TextContent {
                text: format!("Wrote {bytes} bytes to {path}"),
                text_signature: None,
            })],
            details: None,
        })
    }
}
