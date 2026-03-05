use serde_yaml;
use std::path::Path;
use tokio::fs;

#[derive(Debug, Clone)]
pub struct SkillMetadata {
    pub name: String,
    pub description: String,
    pub directory: String,
    pub file_path: String,
}

#[derive(Debug, Clone)]
pub struct ParsedSkill {
    pub meta: SkillMetadata,
    pub instructions: String,
    pub has_scripts: bool,
    pub has_references: bool,
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

async fn dir_exists(path: &Path) -> bool {
    fs::metadata(path).await.map(|m| m.is_dir()).unwrap_or(false)
}

pub async fn discover_skills(agent_dir: &Path) -> Vec<SkillMetadata> {
    let skills_dir = agent_dir.join("skills");
    if !dir_exists(&skills_dir).await {
        return Vec::new();
    }

    let mut entries = match fs::read_dir(&skills_dir).await {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };

    let kebab_re = regex::Regex::new(r"^[a-z0-9]+(-[a-z0-9]+)*$").unwrap();
    let mut skills = Vec::new();

    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let skill_file = path.join("SKILL.md");
        let content = match fs::read_to_string(&skill_file).await {
            Ok(c) => c,
            Err(_) => continue,
        };

        let (fm, _body) = parse_frontmatter(&content);
        let name = fm.get("name").and_then(|v| v.as_str()).map(String::from);
        let description = fm.get("description").and_then(|v| v.as_str()).map(String::from);

        let (name, description) = match (name, description) {
            (Some(n), Some(d)) => (n, d),
            _ => continue,
        };

        let dir_name = entry.file_name().to_string_lossy().to_string();
        if name != dir_name || !kebab_re.is_match(&name) {
            continue;
        }

        skills.push(SkillMetadata {
            name,
            description,
            directory: path.to_string_lossy().to_string(),
            file_path: skill_file.to_string_lossy().to_string(),
        });
    }

    skills.sort_by(|a, b| a.name.cmp(&b.name));
    skills
}

pub async fn load_skill(meta: &SkillMetadata) -> Option<ParsedSkill> {
    let content = fs::read_to_string(&meta.file_path).await.ok()?;
    let (_fm, body) = parse_frontmatter(&content);
    let dir = Path::new(&meta.directory);

    Some(ParsedSkill {
        meta: meta.clone(),
        instructions: body.trim().to_string(),
        has_scripts: dir_exists(&dir.join("scripts")).await,
        has_references: dir_exists(&dir.join("references")).await,
    })
}

pub fn format_skills_for_prompt(skills: &[SkillMetadata]) -> String {
    if skills.is_empty() {
        return String::new();
    }

    let entries: Vec<String> = skills
        .iter()
        .map(|s| {
            format!(
                "<skill>\n<name>{}</name>\n<description>{}</description>\n</skill>",
                s.name, s.description
            )
        })
        .collect();

    format!(
        "# Skills\n\n<available_skills>\n{}\n</available_skills>\n\nWhen a task matches a skill, use the `read` tool to load `skills/<name>/SKILL.md` for full instructions. Scripts within a skill are relative to the skill's directory (e.g., `skills/<name>/scripts/`). Use the `cli` tool to execute them.",
        entries.join("\n")
    )
}

pub async fn expand_skill_command(input: &str, skills: &[SkillMetadata]) -> Option<(String, String)> {
    let re = regex::Regex::new(r"^/skill:([a-z0-9-]+)\s*([\s\S]*)$").unwrap();
    let caps = re.captures(input)?;
    let skill_name = &caps[1];
    let args = caps[2].trim();

    let skill = skills.iter().find(|s| s.name == skill_name)?;
    let parsed = load_skill(skill).await?;

    let mut expanded = format!(
        "<skill name=\"{}\" baseDir=\"{}\">\n{}\n</skill>",
        skill_name, skill.directory, parsed.instructions
    );
    if !args.is_empty() {
        expanded.push_str(&format!("\n\n{args}"));
    }

    Some((expanded, skill_name.to_string()))
}
