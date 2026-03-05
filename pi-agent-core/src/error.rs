use thiserror::Error;

#[derive(Debug, Error)]
pub enum AgentError {
    #[error("Agent is already streaming")]
    AlreadyStreaming,

    #[error("Agent aborted")]
    Aborted,

    #[error("Tool execution failed: {0}")]
    ToolExecution(String),

    #[error("AI error: {0}")]
    AiError(#[from] pi_ai::PiAiError),

    #[error("{0}")]
    Other(String),
}
