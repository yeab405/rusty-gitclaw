use clap::Parser;
use std::path::{Path, PathBuf};
use std::process;

use pi_agent_core::agent::{Agent, AgentOptions};
use pi_agent_core::types::AgentEvent;
use pi_ai::types::AssistantMessageEvent;

mod agents;
mod audit;
mod compliance;
mod config;
mod examples;
mod hooks;
mod knowledge;
mod loader;
mod sandbox;
mod sdk;
mod sdk_hooks;
mod sdk_types;
mod session;
mod skills;
mod tool_loader;
mod tools;
mod voice;
mod workflows;

use audit::{AuditLogger, is_audit_enabled};
use compliance::format_compliance_warnings;
use hooks::{load_hooks_config, run_hooks, HooksFileConfig};
use loader::load_agent;
use session::init_local_session;
use skills::expand_skill_command;
use tool_loader::load_declarative_tools;
use tools::{create_builtin_tools, BuiltinToolsConfig};
use voice::start_voice_server;
use voice::adapter::{VoiceAdapterConfig, VoiceServerOptions};

// ANSI helpers
fn dim(s: &str) -> String {
    format!("\x1b[2m{s}\x1b[0m")
}
fn bold(s: &str) -> String {
    format!("\x1b[1m{s}\x1b[0m")
}
fn red(s: &str) -> String {
    format!("\x1b[31m{s}\x1b[0m")
}
fn green(s: &str) -> String {
    format!("\x1b[32m{s}\x1b[0m")
}

#[derive(Parser, Debug)]
#[command(name = "gitclaw", about = "Git-native AI agent framework")]
struct Cli {
    /// Model to use (provider:model, e.g. anthropic:claude-sonnet-4-5-20250929)
    #[arg(short, long)]
    model: Option<String>,

    /// Agent directory
    #[arg(short, long)]
    dir: Option<String>,

    /// Single-shot prompt (skip REPL)
    #[arg(short, long)]
    prompt: Option<String>,

    /// Environment config name
    #[arg(short, long)]
    env: Option<String>,

    /// Enable sandbox mode (not available in Rust version)
    #[arg(short, long)]
    sandbox: bool,

    /// Remote git repo URL
    #[arg(short, long)]
    repo: Option<String>,

    /// Personal access token for --repo
    #[arg(long)]
    pat: Option<String>,

    /// Session branch name for --repo
    #[arg(long)]
    session: Option<String>,

    /// Enable voice mode (requires OPENAI_API_KEY)
    #[arg(short, long)]
    voice: bool,
}

fn summarize_args(args: &Option<std::collections::HashMap<String, serde_json::Value>>) -> String {
    let args = match args {
        Some(a) => a,
        None => return String::new(),
    };
    if args.is_empty() {
        return String::new();
    }
    args.iter()
        .map(|(k, v)| {
            let s = match v.as_str() {
                Some(s) => s.to_string(),
                None => serde_json::to_string(v).unwrap_or_default(),
            };
            let short = if s.len() > 60 {
                format!("{}…", &s[..60])
            } else {
                s
            };
            format!("{k}: {short}")
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn handle_event(event: &AgentEvent) {
    match event {
        AgentEvent::MessageUpdate {
            assistant_message_event,
        } => {
            if let AssistantMessageEvent::TextDelta { delta, .. } = assistant_message_event {
                eprint!("{delta}");
            }
        }
        AgentEvent::MessageEnd { .. } => {
            eprintln!();
        }
        AgentEvent::ToolExecutionStart {
            tool_name, args, ..
        } => {
            eprintln!("{}", dim(&format!("\n▶ {tool_name}({})", summarize_args(args))));
        }
        AgentEvent::ToolExecutionEnd {
            tool_name,
            result,
            is_error,
            ..
        } => {
            if *is_error {
                eprintln!("{}", red(&format!("✗ {tool_name} failed")));
            } else if let Some(r) = result {
                let text = r
                    .content
                    .first()
                    .and_then(|c| {
                        if let pi_ai::types::ToolResultContent::Text(t) = c {
                            Some(t.text.as_str())
                        } else {
                            None
                        }
                    })
                    .unwrap_or("");
                let preview = if text.len() > 200 {
                    format!("{}…", &text[..200])
                } else {
                    text.to_string()
                };
                if !preview.is_empty() {
                    eprintln!("{}", dim(&preview));
                }
            }
        }
        _ => {}
    }
}

fn is_git_repo(dir: &Path) -> bool {
    std::process::Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .current_dir(dir)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

async fn file_exists(path: &Path) -> bool {
    tokio::fs::metadata(path).await.is_ok()
}

async fn ensure_repo(dir: &Path, model: Option<&str>) -> Result<PathBuf, String> {
    let abs_dir = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());

    // Create directory if needed
    if !file_exists(&abs_dir).await {
        eprintln!("{}", dim(&format!("Creating directory: {}", abs_dir.display())));
        tokio::fs::create_dir_all(&abs_dir)
            .await
            .map_err(|e| format!("Failed to create directory: {e}"))?;
    }

    // Git init if not a repo
    if !is_git_repo(&abs_dir) {
        eprintln!("{}", dim("Initializing git repository..."));
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(&abs_dir)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map_err(|e| format!("git init failed: {e}"))?;

        // Create .gitignore
        let gitignore = abs_dir.join(".gitignore");
        if !file_exists(&gitignore).await {
            tokio::fs::write(&gitignore, "node_modules/\ndist/\n.gitagent/\n")
                .await
                .ok();
        }

        // Initial commit
        let _ = std::process::Command::new("sh")
            .args([
                "-c",
                "git add -A && git commit -m 'Initial commit' --allow-empty",
            ])
            .current_dir(&abs_dir)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }

    // Scaffold agent.yaml if missing
    let agent_yaml = abs_dir.join("agent.yaml");
    if !file_exists(&agent_yaml).await {
        let default_model = model.unwrap_or("openai:gpt-4o-mini");
        let agent_name = abs_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("my-agent");
        let yaml = format!(
            r#"spec_version: "0.1.0"
name: {agent_name}
version: 0.1.0
description: Gitclaw agent for {agent_name}
model:
  preferred: "{default_model}"
  fallback: []
tools: [cli, read, write, memory]
runtime:
  max_turns: 50
"#
        );
        tokio::fs::write(&agent_yaml, yaml).await.ok();
        eprintln!("{}", dim(&format!("Created agent.yaml (model: {default_model})")));
    }

    // Scaffold memory if missing
    let memory_dir = abs_dir.join("memory");
    let memory_file = memory_dir.join("MEMORY.md");
    if !file_exists(&memory_file).await {
        tokio::fs::create_dir_all(&memory_dir).await.ok();
        tokio::fs::write(&memory_file, "# Memory\n").await.ok();
    }

    // Scaffold SOUL.md if missing
    let soul_path = abs_dir.join("SOUL.md");
    if !file_exists(&soul_path).await {
        tokio::fs::write(
            &soul_path,
            "# Identity\n\nYou are a helpful AI agent. You live inside a git repository.\nYou can run commands, read and write files, and remember things.\nBe concise and action-oriented.\n",
        )
        .await
        .ok();
    }

    // Stage new scaffolded files
    let _ = std::process::Command::new("sh")
        .args([
            "-c",
            "git add -A && git diff --cached --quiet || git commit -m 'Scaffold gitclaw agent'",
        ])
        .current_dir(&abs_dir)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();

    Ok(abs_dir)
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    let mut dir = cli
        .dir
        .as_deref()
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

    // Repo mode
    let mut local_session = None;
    if let Some(ref repo_url) = cli.repo {
        if cli.sandbox {
            eprintln!("{}", red("Error: --repo and --sandbox are mutually exclusive"));
            process::exit(1);
        }

        let token = cli
            .pat
            .clone()
            .or_else(|| std::env::var("GITHUB_TOKEN").ok())
            .or_else(|| std::env::var("GIT_TOKEN").ok());
        let token = match token {
            Some(t) => t,
            None => {
                eprintln!(
                    "{}",
                    red("Error: --pat, GITHUB_TOKEN, or GIT_TOKEN is required with --repo")
                );
                process::exit(1);
            }
        };

        // Default dir: /tmp/gitclaw/<repo-name>
        if cli.dir.is_none() {
            let repo_name = repo_url
                .split('/')
                .last()
                .unwrap_or("repo")
                .trim_end_matches(".git");
            dir = PathBuf::from(format!("/tmp/gitclaw/{repo_name}"));
        }

        let sess = match init_local_session(session::LocalRepoOptions {
            url: repo_url.clone(),
            token,
            dir: dir.to_string_lossy().to_string(),
            session: cli.session.clone(),
        }) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("{}", red(&format!("Error: {e}")));
                process::exit(1);
            }
        };
        eprintln!(
            "{}",
            dim(&format!(
                "Local session: {} ({})",
                sess.branch,
                sess.dir.display()
            ))
        );
        dir = sess.dir.clone();
        local_session = Some(sess);
    }

    // Sandbox mode
    if cli.sandbox {
        eprintln!(
            "{}",
            red("Sandbox mode is not yet available in the Rust version of gitclaw.")
        );
        process::exit(1);
    }

    // Ensure repo (skip in repo mode)
    if local_session.is_none() {
        match ensure_repo(&dir, cli.model.as_deref()).await {
            Ok(d) => dir = d,
            Err(e) => {
                eprintln!("{}", red(&format!("Error: {e}")));
                process::exit(1);
            }
        }
    }

    // Voice mode
    if cli.voice {
        let api_key = match std::env::var("OPENAI_API_KEY") {
            Ok(k) => k,
            Err(_) => {
                eprintln!(
                    "{}",
                    red("Error: OPENAI_API_KEY is required for --voice mode")
                );
                process::exit(1);
            }
        };

        let result = start_voice_server(VoiceServerOptions {
            port: None,
            adapter: "openai-realtime".to_string(),
            adapter_config: VoiceAdapterConfig {
                api_key,
                model: None,
                voice: Some("alloy".to_string()),
                instructions: None,
            },
            agent_dir: dir.to_string_lossy().to_string(),
            model: cli.model.clone(),
            env: cli.env.clone(),
        })
        .await;

        match result {
            Ok(cleanup) => {
                // Keep alive until Ctrl+C
                tokio::signal::ctrl_c().await.ok();
                eprintln!("\nDisconnecting...");
                cleanup().await;
            }
            Err(e) => {
                eprintln!("{}", red(&format!("Voice server error: {e}")));
                process::exit(1);
            }
        }
        return;
    }

    // Load agent
    let loaded = match load_agent(&dir, cli.model.as_deref(), cli.env.as_deref()).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("{}", red(&format!("Error: {e}")));
            process::exit(1);
        }
    };

    // Show compliance warnings
    if !loaded.compliance_warnings.is_empty() {
        let yellow = |s: &str| format!("\x1b[33m{s}\x1b[0m");
        eprintln!("{}", yellow("Compliance warnings:"));
        eprintln!("{}", yellow(&format_compliance_warnings(&loaded.compliance_warnings)));
    }

    // Initialize audit logger
    let audit_enabled = is_audit_enabled(loaded.manifest.compliance.as_ref());
    let audit_logger = AuditLogger::new(&loaded.gitagent_dir, &loaded.session_id, audit_enabled);
    if audit_enabled {
        audit_logger.log_session_start().await;
    }

    // Load hooks config
    let hooks_config = load_hooks_config(&loaded.agent_dir).await;

    // Run on_session_start hooks
    if let Some(ref hc) = hooks_config {
        if let Some(ref hook_defs) = hc.hooks.on_session_start {
            let result = run_hooks(
                hook_defs,
                &loaded.agent_dir,
                &serde_json::json!({
                    "event": "on_session_start",
                    "session_id": loaded.session_id,
                    "agent": loaded.manifest.name,
                }),
            )
            .await;
            if result.action == "block" {
                eprintln!(
                    "{}",
                    red(&format!(
                        "Session blocked by hook: {}",
                        result.reason.unwrap_or_else(|| "no reason given".to_string())
                    ))
                );
                process::exit(1);
            }
        }
    }

    // Check API key
    let api_key_env_vars: std::collections::HashMap<&str, &str> = [
        ("anthropic", "ANTHROPIC_API_KEY"),
        ("openai", "OPENAI_API_KEY"),
        ("google", "GOOGLE_API_KEY"),
        ("xai", "XAI_API_KEY"),
        ("groq", "GROQ_API_KEY"),
        ("mistral", "MISTRAL_API_KEY"),
    ]
    .into_iter()
    .collect();

    let provider = &loaded.model.provider;
    if let Some(env_var) = api_key_env_vars.get(provider.as_str()) {
        if std::env::var(env_var).is_err() {
            eprintln!(
                "{}",
                red(&format!("Error: {env_var} environment variable is not set."))
            );
            eprintln!("{}", dim(&format!("Set it with: export {env_var}=your-key-here")));
            process::exit(1);
        }
    }

    // Build tools
    let mut agent_tools = create_builtin_tools(&BuiltinToolsConfig {
        dir: dir.clone(),
        timeout: loaded.manifest.runtime.timeout,
    });

    // Load declarative tools
    let declarative_tools = load_declarative_tools(&loaded.agent_dir).await;
    agent_tools.extend(declarative_tools);

    // Build model options from constraints
    let mut temperature = None;
    let mut max_tokens = None;
    if let Some(ref constraints) = loaded.manifest.model.constraints {
        if let Some(t) = constraints.get("temperature").and_then(|v| v.as_f64()) {
            temperature = Some(t);
        }
        if let Some(t) = constraints.get("max_tokens").and_then(|v| v.as_u64()) {
            max_tokens = Some(t as u32);
        }
    }

    // Print banner
    eprintln!(
        "{}",
        bold(&format!(
            "{} v{}",
            loaded.manifest.name, loaded.manifest.version
        ))
    );
    eprintln!(
        "{}",
        dim(&format!(
            "Model: {}:{}",
            loaded.model.provider, loaded.model.id
        ))
    );
    let tool_names: Vec<&str> = agent_tools.iter().map(|t| t.name()).collect();
    eprintln!("{}", dim(&format!("Tools: {}", tool_names.join(", "))));
    if !loaded.skills.is_empty() {
        let skill_names: Vec<&str> = loaded.skills.iter().map(|s| s.name.as_str()).collect();
        eprintln!("{}", dim(&format!("Skills: {}", skill_names.join(", "))));
    }
    if !loaded.workflows.is_empty() {
        let wf_names: Vec<&str> = loaded.workflows.iter().map(|w| w.name.as_str()).collect();
        eprintln!("{}", dim(&format!("Workflows: {}", wf_names.join(", "))));
    }
    if !loaded.sub_agents.is_empty() {
        let agent_names: Vec<&str> = loaded.sub_agents.iter().map(|a| a.name.as_str()).collect();
        eprintln!("{}", dim(&format!("Agents: {}", agent_names.join(", "))));
    }
    eprintln!(
        "{}",
        dim("Type /skills to list skills, /memory to view memory, /quit to exit\n")
    );

    let agent = Agent::new(AgentOptions {
        system_prompt: Some(loaded.system_prompt),
        model: loaded.model.clone(),
        tools: agent_tools,
        thinking_level: None,
        temperature,
        max_tokens,
    });

    // Subscribe to events
    let _event_handle = agent.subscribe_fn(|event| {
        handle_event(&event);
    });

    // Single-shot mode
    if let Some(ref prompt) = cli.prompt {
        match agent.prompt(prompt).await {
            Ok(()) => {}
            Err(e) => {
                eprintln!("{}", red(&format!("Error: {e}")));
                if let Some(ref hc) = hooks_config {
                    if let Some(ref hook_defs) = hc.hooks.on_error {
                        let _ = run_hooks(
                            hook_defs,
                            &loaded.agent_dir,
                            &serde_json::json!({
                                "event": "on_error",
                                "session_id": loaded.session_id,
                                "error": e.to_string(),
                            }),
                        );
                    }
                }
                process::exit(1);
            }
        }

        if let Some(ref sess) = local_session {
            eprintln!("{}", dim("Finalizing session..."));
            sess.finalize();
        }
        return;
    }

    // REPL mode
    let mut rl = match rustyline::DefaultEditor::new() {
        Ok(rl) => rl,
        Err(e) => {
            eprintln!("{}", red(&format!("Failed to initialize REPL: {e}")));
            process::exit(1);
        }
    };

    loop {
        let readline = rl.readline(&green("→ "));
        match readline {
            Ok(line) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }

                rl.add_history_entry(trimmed).ok();

                if trimmed == "/quit" || trimmed == "/exit" {
                    if let Some(ref sess) = local_session {
                        eprintln!("{}", dim("Finalizing session..."));
                        sess.finalize();
                    }
                    break;
                }

                if trimmed == "/memory" {
                    match tokio::fs::read_to_string(dir.join("memory/MEMORY.md")).await {
                        Ok(mem) => {
                            eprintln!("{}", dim("--- memory ---"));
                            let content = mem.trim();
                            eprintln!("{}", if content.is_empty() { "(empty)" } else { content });
                            eprintln!("{}", dim("--- end ---"));
                        }
                        Err(_) => {
                            eprintln!("{}", dim("(no memory file)"));
                        }
                    }
                    continue;
                }

                if trimmed == "/skills" {
                    if loaded.skills.is_empty() {
                        eprintln!("{}", dim("No skills installed."));
                    } else {
                        for s in &loaded.skills {
                            eprintln!("  {} — {}", bold(&s.name), dim(&s.description));
                        }
                    }
                    continue;
                }

                // Skill expansion
                let mut prompt_text = trimmed.to_string();
                if trimmed.starts_with("/skill:") {
                    match expand_skill_command(trimmed, &loaded.skills).await {
                        Some((expanded, skill_name)) => {
                            eprintln!("{}", dim(&format!("▶ loading skill: {skill_name}")));
                            prompt_text = expanded;
                        }
                        None => {
                            let requested = trimmed
                                .strip_prefix("/skill:")
                                .and_then(|s| s.split_whitespace().next())
                                .unwrap_or("?");
                            eprintln!("{}", red(&format!("Unknown skill: {requested}")));
                            continue;
                        }
                    }
                }

                match agent.prompt(&prompt_text).await {
                    Ok(()) => {}
                    Err(e) => {
                        eprintln!("{}", red(&format!("Error: {e}")));
                    }
                }
            }
            Err(rustyline::error::ReadlineError::Interrupted) => {
                if agent.is_streaming() {
                    agent.abort();
                } else {
                    eprintln!("\nBye!");
                    if let Some(ref sess) = local_session {
                        sess.finalize();
                    }
                    break;
                }
            }
            Err(rustyline::error::ReadlineError::Eof) => {
                break;
            }
            Err(e) => {
                eprintln!("{}", red(&format!("REPL error: {e}")));
                break;
            }
        }
    }
}
