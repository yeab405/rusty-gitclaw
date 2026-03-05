// Programmatic hook wrapping (in-process callbacks)
// Stub for the Rust port - programmatic hooks use the same HookResult type

use crate::hooks::HookResult;
use crate::sdk_types::GCPreToolUseContext;

/// In the SDK, programmatic hooks are passed as closures.
/// This module provides the wrapping logic.

pub type PreToolUseFn = Box<dyn Fn(GCPreToolUseContext) -> HookResult + Send + Sync>;

pub struct ProgrammaticHooks {
    pub pre_tool_use: Option<PreToolUseFn>,
}
