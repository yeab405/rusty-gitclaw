use serde::{Deserialize, Serialize};
use std::path::Path;
use tokio::fs;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeEntry {
    pub path: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default = "default_priority")]
    pub priority: String,
    #[serde(default)]
    pub always_load: bool,
}

fn default_priority() -> String {
    "medium".to_string()
}

#[derive(Debug, Clone, Deserialize)]
struct KnowledgeIndex {
    entries: Vec<KnowledgeEntry>,
}

#[derive(Debug, Clone)]
pub struct LoadedKnowledge {
    pub preloaded: Vec<PreloadedDoc>,
    pub available: Vec<KnowledgeEntry>,
}

#[derive(Debug, Clone)]
pub struct PreloadedDoc {
    pub path: String,
    pub content: String,
}

pub async fn load_knowledge(agent_dir: &Path) -> LoadedKnowledge {
    let knowledge_dir = agent_dir.join("knowledge");
    let index_path = knowledge_dir.join("index.yaml");

    let raw = match fs::read_to_string(&index_path).await {
        Ok(r) => r,
        Err(_) => return LoadedKnowledge { preloaded: vec![], available: vec![] },
    };

    let index: KnowledgeIndex = match serde_yaml::from_str(&raw) {
        Ok(i) => i,
        Err(_) => return LoadedKnowledge { preloaded: vec![], available: vec![] },
    };

    let mut preloaded = Vec::new();
    let mut available = Vec::new();

    for entry in index.entries {
        if entry.always_load {
            if let Ok(content) = fs::read_to_string(knowledge_dir.join(&entry.path)).await {
                preloaded.push(PreloadedDoc {
                    path: entry.path,
                    content: content.trim().to_string(),
                });
            }
        } else {
            available.push(entry);
        }
    }

    LoadedKnowledge { preloaded, available }
}

pub fn format_knowledge_for_prompt(knowledge: &LoadedKnowledge) -> String {
    let mut parts = Vec::new();

    for doc in &knowledge.preloaded {
        parts.push(format!("<knowledge path=\"{}\">\n{}\n</knowledge>", doc.path, doc.content));
    }

    if !knowledge.available.is_empty() {
        let entries: Vec<String> = knowledge.available.iter().map(|e| {
            let tags = if !e.tags.is_empty() {
                format!(" tags=\"{}\"", e.tags.join(","))
            } else {
                String::new()
            };
            format!("<doc path=\"knowledge/{}\" priority=\"{}\"{} />", e.path, e.priority, tags)
        }).collect();
        parts.push(format!(
            "<available_knowledge>\n{}\n</available_knowledge>\n\nUse the `read` tool to load any available knowledge document when needed.",
            entries.join("\n")
        ));
    }

    if parts.is_empty() {
        return String::new();
    }
    format!("# Knowledge\n\n{}", parts.join("\n\n"))
}
