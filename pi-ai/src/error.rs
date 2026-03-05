use thiserror::Error;

#[derive(Debug, Error)]
pub enum PiAiError {
    #[error("No API provider registered for api: {0}")]
    NoProvider(String),

    #[error("Mismatched api: {actual} expected {expected}")]
    MismatchedApi { actual: String, expected: String },

    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("SSE error: {0}")]
    Sse(String),

    #[error("Validation error: {0}")]
    Validation(String),

    #[error("Tool not found: {0}")]
    ToolNotFound(String),

    #[error("API key not found for provider: {0}")]
    ApiKeyNotFound(String),

    #[error("Aborted")]
    Aborted,

    #[error("{0}")]
    Other(String),
}
