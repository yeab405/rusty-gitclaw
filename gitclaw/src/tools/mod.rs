pub mod cli;
pub mod memory;
pub mod read;
pub mod shared;
pub mod write;

use pi_agent_core::BoxedAgentTool;
use std::path::PathBuf;

pub struct BuiltinToolsConfig {
    pub dir: PathBuf,
    pub timeout: Option<u64>,
}

pub fn create_builtin_tools(config: &BuiltinToolsConfig) -> Vec<BoxedAgentTool> {
    vec![
        Box::new(cli::CliTool::new(config.dir.clone(), config.timeout)),
        Box::new(read::ReadTool::new(config.dir.clone())),
        Box::new(write::WriteTool::new(config.dir.clone())),
        Box::new(memory::MemoryTool::new(config.dir.clone())),
    ]
}
