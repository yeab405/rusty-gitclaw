use async_trait::async_trait;
use pi_agent_core::types::{AgentToolResult, AgentToolUpdateCallback};
use pi_ai::types::{TextContent, ToolResultContent};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use tokio_util::sync::CancellationToken;

use super::shared::{paginate_lines, MAX_LINES};

pub struct ReadTool {
    cwd: PathBuf,
}

impl ReadTool {
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

fn is_binary(data: &[u8]) -> bool {
    let check = &data[..data.len().min(8192)];
    check.contains(&0)
}

#[async_trait]
impl pi_agent_core::types::AgentTool for ReadTool {
    fn name(&self) -> &str { "read" }
    fn label(&self) -> &str { "read" }
    fn description(&self) -> &str {
        "Read the contents of a file. Output is limited to 2000 lines or ~100KB. Use offset/limit for large files."
    }
    fn parameters(&self) -> &Value {
        static SCHEMA: once_cell::sync::Lazy<Value> = once_cell::sync::Lazy::new(|| {
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Path to the file to read (relative or absolute)" },
                    "offset": { "type": "number", "description": "Line number to start from (1-indexed)" },
                    "limit": { "type": "number", "description": "Maximum number of lines to read" }
                },
                "required": ["path"]
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
        let offset = args.get("offset").and_then(|v| v.as_u64()).map(|v| v as usize);
        let limit = args.get("limit").and_then(|v| v.as_u64()).map(|v| v as usize);

        let abs_path = self.resolve_path(path);
        let data = tokio::fs::read(&abs_path).await
            .map_err(|e| format!("Failed to read {path}: {e}"))?;

        if is_binary(&data) {
            return Ok(AgentToolResult {
                content: vec![ToolResultContent::Text(TextContent {
                    text: format!("[Binary file: {path} ({} bytes)]", data.len()),
                    text_signature: None,
                })],
                details: None,
            });
        }

        let text = String::from_utf8_lossy(&data);
        let page = paginate_lines(&text, offset, limit)?;
        let mut result = page.text;

        if page.has_more {
            let next_offset = page.shown_range.1 + 1;
            result.push_str(&format!(
                "\n\n[Showing lines {}-{} of {}. Use offset={next_offset} to continue.]",
                page.shown_range.0, page.shown_range.1, page.total_lines
            ));
        }

        Ok(AgentToolResult {
            content: vec![ToolResultContent::Text(TextContent {
                text: result,
                text_signature: None,
            })],
            details: None,
        })
    }
}
