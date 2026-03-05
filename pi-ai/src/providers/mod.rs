pub mod anthropic;
pub mod google;
pub mod google_shared;
pub mod openai_completions;

use std::sync::Arc;

use crate::api_registry::register_api_provider;

/// Register all built-in API providers.
pub fn register_builtins() {
    register_api_provider(
        Arc::new(anthropic::AnthropicProvider),
        None,
    );
    register_api_provider(
        Arc::new(openai_completions::OpenAICompletionsProvider),
        None,
    );
    register_api_provider(
        Arc::new(google::GoogleProvider),
        None,
    );
}
