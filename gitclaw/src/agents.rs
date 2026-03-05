use serde_yaml;
use std::path::Path;
use tokio::fs;

#[derive(Debug, Clone)]
pub struct SubAgentMetadata {
    pub name: String,
    pub description: String,
    pub agent_type: String,
    pub path: String,
}

fn parse_frontmatter(content: &str) -> serde_yaml::Value {
    let re = regex::Regex::new(r"(?s)^---\r?\n(.*?)\r?\n---").unwrap();
    if let Some(caps) = re.captures(content) {
        serde_yaml::from_str(&caps[1]).unwrap_or(serde_yaml::Value::Null)
    } else {
        serde_yaml::Value::Null
    }
}

pub async fn discover_sub_agents(agent_dir: &Path) -> Vec<SubAgentMetadata> {
    let agents_dir = agent_dir.join("agents");
    if !fs::metadata(&agents_dir).await.map(|m| m.is_dir()).unwrap_or(false) {
        return Vec::new();
    }

    let mut entries = match fs::read_dir(&agents_dir).await {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };

    let mut agents = Vec::new();

    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        let file_name = entry.file_name().to_string_lossy().to_string();

        if path.is_dir() {
            let agent_yaml = path.join("agent.yaml");
            if let Ok(raw) = fs::read_to_string(&agent_yaml).await {
                if let Ok(data) = serde_yaml::from_str::<serde_yaml::Value>(&raw) {
                    let name = data.get("name").and_then(|v| v.as_str());
                    let desc = data.get("description").and_then(|v| v.as_str());
                    if let (Some(name), Some(desc)) = (name, desc) {
                        agents.push(SubAgentMetadata {
                            name: name.to_string(),
                            description: desc.to_string(),
                            agent_type: "directory".to_string(),
                            path: format!("agents/{file_name}"),
                        });
                    }
                }
            }
        } else if file_name.ends_with(".md") && path.is_file() {
            if let Ok(raw) = fs::read_to_string(&path).await {
                let fm = parse_frontmatter(&raw);
                let name = fm.get("name").and_then(|v| v.as_str())
                    .map(String::from)
                    .unwrap_or_else(|| file_name.trim_end_matches(".md").to_string());
                let desc = fm.get("description").and_then(|v| v.as_str()).map(String::from);
                if let Some(desc) = desc {
                    agents.push(SubAgentMetadata {
                        name,
                        description: desc,
                        agent_type: "file".to_string(),
                        path: format!("agents/{file_name}"),
                    });
                }
            }
        }
    }

    agents.sort_by(|a, b| a.name.cmp(&b.name));
    agents
}

pub fn format_sub_agents_for_prompt(agents: &[SubAgentMetadata]) -> String {
    if agents.is_empty() {
        return String::new();
    }

    let entries: Vec<String> = agents
        .iter()
        .map(|a| {
            format!(
                "<agent>\n<name>{}</name>\n<description>{}</description>\n<type>{}</type>\n<path>{}</path>\n</agent>",
                a.name, a.description, a.agent_type, a.path
            )
        })
        .collect();

    format!(
        "# Sub-Agents\n\n<available_agents>\n{}\n</available_agents>\n\nTo delegate to a sub-agent, use the `cli` tool to run: `gitclaw --dir {{agent_path}} -p \"task description\"`\nFor file-based agents, use the `read` tool to load their instructions.",
        entries.join("\n")
    )
}
