use serde_yaml;
use std::path::Path;
use tokio::fs;

#[derive(Debug, Clone)]
pub struct WorkflowMetadata {
    pub name: String,
    pub description: String,
    pub file_path: String,
    pub format: String,
}

fn parse_frontmatter(content: &str) -> (serde_yaml::Value, String) {
    let re = regex::Regex::new(r"(?s)^---\r?\n(.*?)\r?\n---\r?\n?(.*)$").unwrap();
    if let Some(caps) = re.captures(content) {
        let fm: serde_yaml::Value = serde_yaml::from_str(&caps[1]).unwrap_or(serde_yaml::Value::Null);
        (fm, caps[2].to_string())
    } else {
        (serde_yaml::Value::Null, content.to_string())
    }
}

pub async fn discover_workflows(agent_dir: &Path) -> Vec<WorkflowMetadata> {
    let workflows_dir = agent_dir.join("workflows");
    if !fs::metadata(&workflows_dir).await.map(|m| m.is_dir()).unwrap_or(false) {
        return Vec::new();
    }

    let mut entries = match fs::read_dir(&workflows_dir).await {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };

    let mut workflows = Vec::new();

    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }

        let file_name = entry.file_name().to_string_lossy().to_string();

        if file_name.ends_with(".yaml") || file_name.ends_with(".yml") {
            if let Ok(raw) = fs::read_to_string(&path).await {
                if let Ok(data) = serde_yaml::from_str::<serde_yaml::Value>(&raw) {
                    let name = data.get("name").and_then(|v| v.as_str());
                    let desc = data.get("description").and_then(|v| v.as_str());
                    if let (Some(name), Some(desc)) = (name, desc) {
                        workflows.push(WorkflowMetadata {
                            name: name.to_string(),
                            description: desc.to_string(),
                            file_path: format!("workflows/{file_name}"),
                            format: "yaml".to_string(),
                        });
                    }
                }
            }
        } else if file_name.ends_with(".md") {
            if let Ok(raw) = fs::read_to_string(&path).await {
                let (fm, _body) = parse_frontmatter(&raw);
                let name = fm.get("name").and_then(|v| v.as_str())
                    .map(String::from)
                    .unwrap_or_else(|| file_name.trim_end_matches(".md").to_string());
                let desc = fm.get("description").and_then(|v| v.as_str()).map(String::from);
                if let Some(desc) = desc {
                    workflows.push(WorkflowMetadata {
                        name,
                        description: desc,
                        file_path: format!("workflows/{file_name}"),
                        format: "markdown".to_string(),
                    });
                }
            }
        }
    }

    workflows.sort_by(|a, b| a.name.cmp(&b.name));
    workflows
}

pub fn format_workflows_for_prompt(workflows: &[WorkflowMetadata]) -> String {
    if workflows.is_empty() {
        return String::new();
    }

    let entries: Vec<String> = workflows
        .iter()
        .map(|w| {
            format!(
                "<workflow>\n<name>{}</name>\n<description>{}</description>\n<path>{}</path>\n</workflow>",
                w.name, w.description, w.file_path
            )
        })
        .collect();

    format!(
        "# Workflows\n\n<available_workflows>\n{}\n</available_workflows>\n\nUse the `read` tool to load a workflow's full definition when you need to follow it.",
        entries.join("\n")
    )
}
