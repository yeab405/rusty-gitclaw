/// Sandbox provider trait and stub implementation.
/// The TS version uses gitmachine (a JS package). For the Rust port,
/// we define the trait but only provide a stub that prints an error.

pub trait SandboxProvider: Send + Sync {
    fn start(&self) -> Result<(), String>;
    fn stop(&self) -> Result<(), String>;
    fn run(&self, command: &str) -> Result<String, String>;
    fn read_file(&self, path: &str) -> Result<String, String>;
    fn write_file(&self, path: &str, content: &str) -> Result<(), String>;
    fn repo_path(&self) -> &str;
}

pub struct StubSandbox;

impl SandboxProvider for StubSandbox {
    fn start(&self) -> Result<(), String> {
        Err("Sandbox mode is not yet available in the Rust version of gitclaw. Use the TypeScript version for sandbox support.".to_string())
    }

    fn stop(&self) -> Result<(), String> {
        Ok(())
    }

    fn run(&self, _command: &str) -> Result<String, String> {
        Err("Sandbox not available".to_string())
    }

    fn read_file(&self, _path: &str) -> Result<String, String> {
        Err("Sandbox not available".to_string())
    }

    fn write_file(&self, _path: &str, _content: &str) -> Result<(), String> {
        Err("Sandbox not available".to_string())
    }

    fn repo_path(&self) -> &str {
        ""
    }
}
