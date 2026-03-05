use crate::api_registry::get_api_provider;
use crate::error::PiAiError;
use crate::event_stream::AssistantMessageEventStream;
use crate::providers;
use crate::types::*;

use std::sync::Once;

static INIT: Once = Once::new();

fn ensure_builtins() {
    INIT.call_once(|| {
        providers::register_builtins();
    });
}

/// Stream a response from the model's API provider.
pub fn stream(
    model: &Model,
    context: &Context,
    options: &StreamOptions,
) -> Result<AssistantMessageEventStream, PiAiError> {
    ensure_builtins();
    let provider = get_api_provider(&model.api)
        .ok_or_else(|| PiAiError::NoProvider(model.api.clone()))?;
    let (stream, _sender) = provider.stream(model, context, options);
    Ok(stream)
}

/// Complete (non-streaming) - stream and collect the final result.
pub async fn complete(
    model: &Model,
    context: &Context,
    options: &StreamOptions,
) -> Result<AssistantMessage, PiAiError> {
    let s = stream(model, context, options)?;
    s.result()
        .await
        .ok_or_else(|| PiAiError::Other("Stream ended without result".to_string()))
}

/// Stream with simplified options (auto-configures thinking/reasoning).
pub fn stream_simple(
    model: &Model,
    context: &Context,
    options: &SimpleStreamOptions,
) -> Result<AssistantMessageEventStream, PiAiError> {
    ensure_builtins();
    let provider = get_api_provider(&model.api)
        .ok_or_else(|| PiAiError::NoProvider(model.api.clone()))?;
    let (stream, _sender) = provider.stream_simple(model, context, options);
    Ok(stream)
}

/// Complete with simplified options.
pub async fn complete_simple(
    model: &Model,
    context: &Context,
    options: &SimpleStreamOptions,
) -> Result<AssistantMessage, PiAiError> {
    let s = stream_simple(model, context, options)?;
    s.result()
        .await
        .ok_or_else(|| PiAiError::Other("Stream ended without result".to_string()))
}
