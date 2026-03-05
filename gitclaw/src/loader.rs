use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tokio::fs;
use uuid::Uuid;

use pi_ai::types::Model;

use crate::agents::{discover_sub_agents, format_sub_agents_for_prompt, SubAgentMetadata};
use crate::compliance::{
    load_compliance_context, validate_compliance, ComplianceWarning,
};
use crate::config::{load_env_config, EnvConfig};
use crate::examples::{format_examples_for_prompt, load_examples, ExampleEntry};
use crate::knowledge::{format_knowledge_for_prompt, load_knowledge, LoadedKnowledge};
use crate::skills::{discover_skills, format_skills_for_prompt, SkillMetadata};
use crate::workflows::{discover_workflows, format_workflows_for_prompt, WorkflowMetadata};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentManifest {
    pub spec_version: String,
    pub name: String,
    pub version: String,
    pub description: String,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    #[serde(default)]
    pub metadata: Option<serde_yaml::Value>,
    pub model: ManifestModel,
    pub tools: Vec<String>,
    #[serde(default)]
    pub skills: Option<Vec<String>>,
    pub runtime: ManifestRuntime,
    #[serde(default, rename = "extends")]
    pub extends_from: Option<String>,
    #[serde(default)]
    pub dependencies: Option<Vec<ManifestDependency>>,
    #[serde(default)]
    pub agents: Option<serde_yaml::Value>,
    #[serde(default)]
    pub delegation: Option<serde_yaml::Value>,
    #[serde(default)]
    pub compliance: Option<serde_yaml::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestModel {
    pub preferred: String,
    #[serde(default)]
    pub fallback: Vec<String>,
    #[serde(default)]
    pub constraints: Option<serde_yaml::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestRuntime {
    pub max_turns: u32,
    #[serde(default)]
    pub timeout: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestDependency {
    pub name: String,
    pub source: String,
    pub version: String,
    pub mount: String,
}

pub struct LoadedAgent {
    pub system_prompt: String,
    pub manifest: AgentManifest,
    pub model: Model,
    pub skills: Vec<SkillMetadata>,
    pub knowledge: LoadedKnowledge,
    pub workflows: Vec<WorkflowMetadata>,
    pub sub_agents: Vec<SubAgentMetadata>,
    pub examples: Vec<ExampleEntry>,
    pub env_config: EnvConfig,
    pub session_id: String,
    pub agent_dir: PathBuf,
    pub gitagent_dir: PathBuf,
    pub compliance_warnings: Vec<ComplianceWarning>,
}

async fn read_file_or(path: &Path, fallback: &str) -> String {
    fs::read_to_string(path).await.unwrap_or_else(|_| fallback.to_string())
}

fn parse_model_string(model_str: &str) -> Result<(String, String), String> {
    match model_str.find(':') {
        Some(idx) => Ok((model_str[..idx].to_string(), model_str[idx + 1..].to_string())),
        None => Err(format!(
            "Invalid model format: \"{model_str}\". Expected \"provider:model\" (e.g., \"anthropic:claude-sonnet-4-5-20250929\")"
        )),
    }
}

async fn ensure_gitagent_dir(agent_dir: &Path) -> PathBuf {
    let gitagent_dir = agent_dir.join(".gitagent");
    let _ = fs::create_dir_all(&gitagent_dir).await;

    // Ensure .gitagent is in .gitignore
    let gitignore_path = agent_dir.join(".gitignore");
    if let Ok(gitignore) = fs::read_to_string(&gitignore_path).await {
        if !gitignore.contains(".gitagent") {
            let new = format!("{}\n.gitagent/\n", gitignore.trim_end());
            let _ = fs::write(&gitignore_path, new).await;
        }
    }

    gitagent_dir
}

async fn write_session_state(gitagent_dir: &Path) -> String {
    let session_id = Uuid::new_v4().to_string();
    let state = serde_json::json!({
        "session_id": session_id,
        "started_at": chrono_now(),
    });
    let _ = fs::write(
        gitagent_dir.join("state.json"),
        serde_json::to_string_pretty(&state).unwrap_or_default(),
    )
    .await;
    session_id
}

fn chrono_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let dur = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}Z", dur.as_secs())
}

pub async fn load_agent(
    agent_dir: &Path,
    model_flag: Option<&str>,
    env_flag: Option<&str>,
) -> Result<LoadedAgent, String> {
    let manifest_raw = fs::read_to_string(agent_dir.join("agent.yaml"))
        .await
        .map_err(|e| format!("Failed to read agent.yaml: {e}"))?;
    let manifest: AgentManifest = serde_yaml::from_str(&manifest_raw)
        .map_err(|e| format!("Failed to parse agent.yaml: {e}"))?;

    let env_config = load_env_config(agent_dir, env_flag).await;
    let gitagent_dir = ensure_gitagent_dir(agent_dir).await;
    let session_id = write_session_state(&gitagent_dir).await;

    let compliance_warnings = validate_compliance(&manifest);

    // Read identity files
    let soul = read_file_or(&agent_dir.join("SOUL.md"), "").await;
    let rules = read_file_or(&agent_dir.join("RULES.md"), "").await;
    let duties = read_file_or(&agent_dir.join("DUTIES.md"), "").await;
    let agents_md = read_file_or(&agent_dir.join("AGENTS.md"), "").await;

    // Build system prompt
    let mut parts = Vec::new();
    parts.push(format!(
        "# {} v{}\n{}",
        manifest.name, manifest.version, manifest.description
    ));

    if !soul.is_empty() {
        parts.push(soul);
    }
    if !rules.is_empty() {
        parts.push(rules);
    }
    if !duties.is_empty() {
        parts.push(duties);
    }
    if !agents_md.is_empty() {
        parts.push(agents_md);
    }

    parts.push("# Memory\n\nYou have a memory file at memory/MEMORY.md. Use the `memory` tool to load and save memories. Each save creates a git commit, so your memory has full history. You can also use the `cli` tool to run git commands for deeper memory inspection (git log, git diff, git show).".to_string());

    // Discover modules
    let knowledge = load_knowledge(agent_dir).await;
    let kb = format_knowledge_for_prompt(&knowledge);
    if !kb.is_empty() {
        parts.push(kb);
    }

    let mut skills = discover_skills(agent_dir).await;
    if let Some(ref allowed) = manifest.skills {
        let allowed_set: std::collections::HashSet<&str> =
            allowed.iter().map(|s| s.as_str()).collect();
        skills.retain(|s| allowed_set.contains(s.name.as_str()));
    }
    let sb = format_skills_for_prompt(&skills);
    if !sb.is_empty() {
        parts.push(sb);
    }

    let workflows = discover_workflows(agent_dir).await;
    let wb = format_workflows_for_prompt(&workflows);
    if !wb.is_empty() {
        parts.push(wb);
    }

    let sub_agents = discover_sub_agents(agent_dir).await;
    let ab = format_sub_agents_for_prompt(&sub_agents);
    if !ab.is_empty() {
        parts.push(ab);
    }

    let examples = load_examples(agent_dir).await;
    let eb = format_examples_for_prompt(&examples);
    if !eb.is_empty() {
        parts.push(eb);
    }

    let cb = load_compliance_context(agent_dir).await;
    if !cb.is_empty() {
        parts.push(cb);
    }

    let system_prompt = parts.join("\n\n");

    // Resolve model
    let model_str = env_config
        .model_override
        .as_deref()
        .or(model_flag)
        .unwrap_or(&manifest.model.preferred);

    if model_str.is_empty() {
        return Err("No model configured. Either:\n  - Set model.preferred in agent.yaml (e.g., \"anthropic:claude-sonnet-4-5-20250929\")\n  - Pass --model provider:model on the command line".to_string());
    }

    let (provider, model_id) = parse_model_string(model_str)?;
    let model = pi_ai::get_model(&provider, &model_id).ok_or_else(|| {
        format!("Model not found: {provider}:{model_id}")
    })?;

    Ok(LoadedAgent {
        system_prompt,
        manifest,
        model,
        skills,
        knowledge,
        workflows,
        sub_agents,
        examples,
        env_config,
        session_id,
        agent_dir: agent_dir.to_path_buf(),
        gitagent_dir,
        compliance_warnings,
    })
}
