use std::path::Path;
use tokio::fs;

#[derive(Debug, Clone)]
pub struct ExampleEntry {
    pub name: String,
    pub content: String,
}

pub async fn load_examples(agent_dir: &Path) -> Vec<ExampleEntry> {
    let examples_dir = agent_dir.join("examples");
    if !fs::metadata(&examples_dir).await.map(|m| m.is_dir()).unwrap_or(false) {
        return Vec::new();
    }

    let mut entries = match fs::read_dir(&examples_dir).await {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };

    let mut examples = Vec::new();

    while let Ok(Some(entry)) = entries.next_entry().await {
        let file_name = entry.file_name().to_string_lossy().to_string();
        if !file_name.ends_with(".md") {
            continue;
        }

        if let Ok(content) = fs::read_to_string(entry.path()).await {
            examples.push(ExampleEntry {
                name: file_name.trim_end_matches(".md").to_string(),
                content: content.trim().to_string(),
            });
        }
    }

    examples.sort_by(|a, b| a.name.cmp(&b.name));
    examples
}

pub fn format_examples_for_prompt(examples: &[ExampleEntry]) -> String {
    if examples.is_empty() {
        return String::new();
    }

    let blocks: Vec<String> = examples
        .iter()
        .map(|e| format!("<example name=\"{}\">\n{}\n</example>", e.name, e.content))
        .collect();

    format!("# Examples\n\n<examples>\n{}\n</examples>", blocks.join("\n\n"))
}
