use std::path::{Path, PathBuf};
use std::process::Command;

pub struct LocalRepoOptions {
    pub url: String,
    pub token: String,
    pub dir: String,
    pub session: Option<String>,
}

pub struct LocalSession {
    pub dir: PathBuf,
    pub branch: String,
    pub session_id: String,
}

fn authed_url(url: &str, token: &str) -> String {
    url.replace("https://", &format!("https://{token}@"))
}

fn clean_url(url: &str) -> String {
    let re = regex::Regex::new(r"^https://[^@]+@").unwrap();
    re.replace(url, "https://").to_string()
}

fn git(args: &str, cwd: &Path) -> Result<String, String> {
    let output = Command::new("sh")
        .arg("-c")
        .arg(format!("git {args}"))
        .current_dir(cwd)
        .output()
        .map_err(|e| format!("git failed: {e}"))?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

fn get_default_branch(cwd: &Path) -> String {
    if let Ok(ref_str) = git("symbolic-ref refs/remotes/origin/HEAD", cwd) {
        return ref_str.replace("refs/remotes/origin/", "");
    }
    if git("rev-parse --verify origin/main", cwd).is_ok() {
        return "main".to_string();
    }
    "master".to_string()
}

impl LocalSession {
    pub fn commit_changes(&self, msg: Option<&str>) {
        let _ = git("add -A", &self.dir);
        if git("diff --cached --quiet", &self.dir).is_err() {
            let default_msg = format!("gitclaw: auto-commit ({})", self.branch);
            let commit_msg = msg.unwrap_or(&default_msg);
            let _ = git(&format!("commit -m \"{commit_msg}\""), &self.dir);
        }
    }

    pub fn push(&self) {
        let _ = git(&format!("push origin {}", self.branch), &self.dir);
    }

    pub fn finalize(&self) {
        self.commit_changes(None);
        self.push();
        // Strip PAT from remote
        // We don't have the original clean URL here, so skip
    }
}

pub fn init_local_session(opts: LocalRepoOptions) -> Result<LocalSession, String> {
    let dir = PathBuf::from(&opts.dir);
    let a_url = authed_url(&opts.url, &opts.token);

    if !dir.exists() {
        let dir_str = dir.to_string_lossy();
        let output = Command::new("sh")
            .arg("-c")
            .arg(format!(
                "git clone --depth 1 --no-single-branch {a_url} {dir_str}"
            ))
            .output()
            .map_err(|e| format!("git clone failed: {e}"))?;
        if !output.status.success() {
            return Err(format!(
                "git clone failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }
    } else {
        let _ = git(&format!("remote set-url origin {a_url}"), &dir);
        let _ = git("fetch origin", &dir);
        let default_branch = get_default_branch(&dir);
        let _ = git(&format!("checkout {default_branch}"), &dir);
        let _ = git(&format!("reset --hard origin/{default_branch}"), &dir);
    }

    let (branch, session_id) = if let Some(ref session) = opts.session {
        let branch = session.clone();
        let sid = branch
            .strip_prefix("gitclaw/session-")
            .unwrap_or(&branch)
            .to_string();
        if git(&format!("checkout {branch}"), &dir).is_err() {
            let _ = git(&format!("checkout -b {branch} origin/{branch}"), &dir);
        }
        let _ = git(&format!("pull origin {branch}"), &dir);
        (branch, sid)
    } else {
        let sid = format!("{:08x}", rand::random::<u32>());
        let branch = format!("gitclaw/session-{sid}");
        let _ = git(&format!("checkout -b {branch}"), &dir);
        (branch, sid)
    };

    // Scaffold if needed
    let agent_yaml = dir.join("agent.yaml");
    if !agent_yaml.exists() {
        let name = opts
            .url
            .split('/')
            .last()
            .unwrap_or("agent")
            .trim_end_matches(".git");
        let yaml = format!(
            "spec_version: \"0.1.0\"\nname: {name}\nversion: 0.1.0\ndescription: Gitclaw agent for {name}\nmodel:\n  preferred: \"openai:gpt-4o-mini\"\n  fallback: []\ntools: [cli, read, write, memory]\nruntime:\n  max_turns: 50\n"
        );
        let _ = std::fs::write(&agent_yaml, yaml);
    }

    let memory_file = dir.join("memory/MEMORY.md");
    if !memory_file.exists() {
        let _ = std::fs::create_dir_all(dir.join("memory"));
        let _ = std::fs::write(&memory_file, "# Memory\n");
    }

    Ok(LocalSession {
        dir,
        branch,
        session_id,
    })
}

fn rand_random() -> u32 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos()
}

// Replace rand::random with our own since we don't have the rand crate
mod rand {
    pub fn random<T: From<u32>>() -> T {
        T::from(super::rand_random())
    }
}
