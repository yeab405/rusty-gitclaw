use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use tokio::fs;
use tokio::process::Command;
use tokio::io::AsyncWriteExt;

#[derive(Debug, Clone, Deserialize)]
pub struct HookDefinition {
    pub script: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HooksFileConfig {
    pub hooks: HooksMap,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct HooksMap {
    pub on_session_start: Option<Vec<HookDefinition>>,
    pub pre_tool_use: Option<Vec<HookDefinition>>,
    pub post_response: Option<Vec<HookDefinition>>,
    pub on_error: Option<Vec<HookDefinition>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookResult {
    pub action: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<HashMap<String, serde_json::Value>>,
}

pub async fn load_hooks_config(agent_dir: &Path) -> Option<HooksFileConfig> {
    let hooks_path = agent_dir.join("hooks").join("hooks.yaml");
    let raw = fs::read_to_string(&hooks_path).await.ok()?;
    let config: HooksFileConfig = serde_yaml::from_str(&raw).ok()?;
    if config.hooks.on_session_start.is_none()
        && config.hooks.pre_tool_use.is_none()
        && config.hooks.post_response.is_none()
        && config.hooks.on_error.is_none()
    {
        return None;
    }
    Some(config)
}

async fn execute_hook(
    hook: &HookDefinition,
    agent_dir: &Path,
    input: &serde_json::Value,
) -> Result<HookResult, String> {
    let script_path = agent_dir.join("hooks").join(&hook.script);

    let mut child = Command::new("sh")
        .arg(script_path.to_string_lossy().as_ref())
        .current_dir(agent_dir)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("Hook \"{}\" failed to start: {e}", hook.script))?;

    if let Some(mut stdin) = child.stdin.take() {
        let json = serde_json::to_string(input).unwrap_or_default();
        let _ = stdin.write_all(json.as_bytes()).await;
        drop(stdin);
    }

    let output = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        child.wait_with_output(),
    )
    .await
    .map_err(|_| format!("Hook \"{}\" timed out after 10s", hook.script))?
    .map_err(|e| format!("Hook \"{}\" failed: {e}", hook.script))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "Hook \"{}\" exited with code {}: {}",
            hook.script,
            output.status.code().unwrap_or(-1),
            stderr.trim()
        ));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    match serde_json::from_str::<HookResult>(stdout.trim()) {
        Ok(result) => Ok(result),
        Err(_) => Ok(HookResult {
            action: "allow".to_string(),
            reason: None,
            args: None,
        }),
    }
}

pub async fn run_hooks(
    hooks: &[HookDefinition],
    agent_dir: &Path,
    input: &serde_json::Value,
) -> HookResult {
    if hooks.is_empty() {
        return HookResult {
            action: "allow".to_string(),
            reason: None,
            args: None,
        };
    }

    for hook in hooks {
        match execute_hook(hook, agent_dir, input).await {
            Ok(result) => {
                if result.action == "block" || result.action == "modify" {
                    return result;
                }
            }
            Err(e) => {
                eprintln!("Hook error: {e}");
            }
        }
    }

    HookResult {
        action: "allow".to_string(),
        reason: None,
        args: None,
    }
}
