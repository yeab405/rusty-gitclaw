use serde::Serialize;
use serde_json;
use std::path::{Path, PathBuf};
use tokio::fs;

#[derive(Debug, Serialize)]
pub struct AuditEntry {
    pub timestamp: String,
    pub session_id: String,
    pub event: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub struct AuditLogger {
    log_path: PathBuf,
    session_id: String,
    enabled: bool,
}

impl AuditLogger {
    pub fn new(gitagent_dir: &Path, session_id: &str, enabled: bool) -> Self {
        Self {
            log_path: gitagent_dir.join("audit.jsonl"),
            session_id: session_id.to_string(),
            enabled,
        }
    }

    pub async fn log(&self, event: &str, extra: Option<AuditEntry>) {
        if !self.enabled {
            return;
        }

        let entry = extra.unwrap_or(AuditEntry {
            timestamp: chrono_now(),
            session_id: self.session_id.clone(),
            event: event.to_string(),
            tool: None,
            args: None,
            result: None,
            error: None,
        });

        if let Ok(json) = serde_json::to_string(&entry) {
            if let Some(parent) = self.log_path.parent() {
                let _ = fs::create_dir_all(parent).await;
            }
            let _ = append_line(&self.log_path, &json).await;
        }
    }

    pub async fn log_tool_use(&self, tool: &str, args: &serde_json::Value) {
        self.log("tool_use", Some(AuditEntry {
            timestamp: chrono_now(),
            session_id: self.session_id.clone(),
            event: "tool_use".to_string(),
            tool: Some(tool.to_string()),
            args: Some(args.clone()),
            result: None,
            error: None,
        })).await;
    }

    pub async fn log_tool_result(&self, tool: &str, result: &str) {
        let truncated = if result.len() > 1000 { &result[..1000] } else { result };
        self.log("tool_result", Some(AuditEntry {
            timestamp: chrono_now(),
            session_id: self.session_id.clone(),
            event: "tool_result".to_string(),
            tool: Some(tool.to_string()),
            args: None,
            result: Some(truncated.to_string()),
            error: None,
        })).await;
    }

    pub async fn log_response(&self) {
        self.log("response", None).await;
    }

    pub async fn log_error(&self, error: &str) {
        self.log("error", Some(AuditEntry {
            timestamp: chrono_now(),
            session_id: self.session_id.clone(),
            event: "error".to_string(),
            tool: None,
            args: None,
            result: None,
            error: Some(error.to_string()),
        })).await;
    }

    pub async fn log_session_start(&self) {
        self.log("session_start", None).await;
    }

    pub async fn log_session_end(&self) {
        self.log("session_end", None).await;
    }
}

pub fn is_audit_enabled(compliance: Option<&serde_yaml::Value>) -> bool {
    compliance
        .and_then(|c| c.get("recordkeeping"))
        .and_then(|r| r.get("audit_logging"))
        .and_then(|a| a.as_bool())
        .unwrap_or(false)
}

fn chrono_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let dur = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = dur.as_secs();
    // Simple ISO format
    format!("{secs}")
}

async fn append_line(path: &Path, line: &str) {
    use tokio::io::AsyncWriteExt;
    if let Ok(mut file) = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await
    {
        let _ = file.write_all(line.as_bytes()).await;
        let _ = file.write_all(b"\n").await;
    }
}
