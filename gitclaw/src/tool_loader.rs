use async_trait::async_trait;
use pi_agent_core::types::{AgentToolResult, AgentToolUpdateCallback};
use pi_ai::types::{TextContent, ToolResultContent};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tokio::fs;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Deserialize)]
struct ToolDefinition {
    name: String,
    description: String,
    input_schema: Value,
    implementation: ToolImplementation,
}

#[derive(Debug, Deserialize)]
struct ToolImplementation {
    script: String,
    runtime: Option<String>,
}

/// Build a JSON schema Value from a simplified input_schema definition.
pub fn build_json_schema(schema: &Value) -> Value {
    // For the Rust port, we pass through the JSON schema as-is
    // The TS version converts to TypeBox, but we use raw JSON Schema
    schema.clone()
}

struct DeclarativeTool {
    name: String,
    description: String,
    schema: Value,
    script_path: PathBuf,
    runtime: String,
    agent_dir: PathBuf,
}

#[async_trait]
impl pi_agent_core::types::AgentTool for DeclarativeTool {
    fn name(&self) -> &str {
        &self.name
    }
    fn label(&self) -> &str {
        &self.name
    }
    fn description(&self) -> &str {
        &self.description
    }
    fn parameters(&self) -> &Value {
        &self.schema
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

        let mut child = Command::new(&self.runtime)
            .arg(self.script_path.to_string_lossy().as_ref())
            .current_dir(&self.agent_dir)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| format!("Tool \"{}\" failed to start: {e}", self.name))?;

        if let Some(mut stdin) = child.stdin.take() {
            let json = serde_json::to_string(&args).unwrap_or_default();
            let _ = stdin.write_all(json.as_bytes()).await;
            drop(stdin);
        }

        let output = tokio::time::timeout(
            std::time::Duration::from_secs(120),
            child.wait_with_output(),
        )
        .await
        .map_err(|_| format!("Tool \"{}\" timed out after 120s", self.name))?
        .map_err(|e| format!("Tool \"{}\" failed: {e}", self.name))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!(
                "Tool \"{}\" exited with code {}: {}",
                self.name,
                output.status.code().unwrap_or(-1),
                stderr.trim()
            ));
        }

        let mut text = String::from_utf8_lossy(&output.stdout).trim().to_string();

        // Try parsing JSON output
        if let Ok(parsed) = serde_json::from_str::<Value>(&text) {
            if let Some(t) = parsed.get("text").and_then(|v| v.as_str()) {
                text = t.to_string();
            } else if let Some(r) = parsed.get("result") {
                text = match r.as_str() {
                    Some(s) => s.to_string(),
                    None => serde_json::to_string(r).unwrap_or_default(),
                };
            }
        }

        if text.is_empty() {
            text = "(no output)".to_string();
        }

        Ok(AgentToolResult {
            content: vec![ToolResultContent::Text(TextContent {
                text,
                text_signature: None,
            })],
            details: None,
        })
    }
}

pub async fn load_declarative_tools(
    agent_dir: &Path,
) -> Vec<Box<dyn pi_agent_core::types::AgentTool>> {
    let tools_dir = agent_dir.join("tools");
    if !fs::metadata(&tools_dir)
        .await
        .map(|m| m.is_dir())
        .unwrap_or(false)
    {
        return Vec::new();
    }

    let mut entries = match fs::read_dir(&tools_dir).await {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };

    let mut tools: Vec<Box<dyn pi_agent_core::types::AgentTool>> = Vec::new();

    while let Ok(Some(entry)) = entries.next_entry().await {
        let file_name = entry.file_name().to_string_lossy().to_string();
        if !file_name.ends_with(".yaml") && !file_name.ends_with(".yml") {
            continue;
        }

        if let Ok(raw) = fs::read_to_string(entry.path()).await {
            if let Ok(def) = serde_yaml::from_str::<ToolDefinition>(&raw) {
                let script_path = tools_dir.join(&def.implementation.script);
                tools.push(Box::new(DeclarativeTool {
                    name: def.name,
                    description: def.description,
                    schema: build_json_schema(&def.input_schema),
                    script_path,
                    runtime: def.implementation.runtime.unwrap_or_else(|| "sh".to_string()),
                    agent_dir: agent_dir.to_path_buf(),
                }));
            }
        }
    }

    tools
}
