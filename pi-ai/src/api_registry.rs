use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use crate::event_stream::{AssistantMessageEventSender, AssistantMessageEventStream};
use crate::types::{Context, Model, SimpleStreamOptions, StreamOptions};

/// Trait for API providers (Anthropic, OpenAI, Google, etc.)
#[async_trait]
pub trait ApiProvider: Send + Sync {
    fn api(&self) -> &str;

    fn stream(
        &self,
        model: &Model,
        context: &Context,
        options: &StreamOptions,
    ) -> (AssistantMessageEventStream, AssistantMessageEventSender);

    fn stream_simple(
        &self,
        model: &Model,
        context: &Context,
        options: &SimpleStreamOptions,
    ) -> (AssistantMessageEventStream, AssistantMessageEventSender);
}

/// Wrapper that holds a provider and optional source ID for unregistration.
struct RegisteredProvider {
    provider: Arc<dyn ApiProvider>,
    #[allow(dead_code)]
    source_id: Option<String>,
}

/// Global API provider registry.
static REGISTRY: once_cell::sync::Lazy<RwLock<HashMap<String, RegisteredProvider>>> =
    once_cell::sync::Lazy::new(|| RwLock::new(HashMap::new()));

pub fn register_api_provider(provider: Arc<dyn ApiProvider>, source_id: Option<String>) {
    let api = provider.api().to_string();
    let mut reg = REGISTRY.write().unwrap();
    reg.insert(
        api,
        RegisteredProvider {
            provider,
            source_id,
        },
    );
}

pub fn get_api_provider(api: &str) -> Option<Arc<dyn ApiProvider>> {
    let reg = REGISTRY.read().unwrap();
    reg.get(api).map(|r| Arc::clone(&r.provider))
}

pub fn get_api_providers() -> Vec<Arc<dyn ApiProvider>> {
    let reg = REGISTRY.read().unwrap();
    reg.values().map(|r| Arc::clone(&r.provider)).collect()
}

pub fn unregister_api_providers(source_id: &str) {
    let mut reg = REGISTRY.write().unwrap();
    reg.retain(|_, v| v.source_id.as_deref() != Some(source_id));
}

pub fn clear_api_providers() {
    let mut reg = REGISTRY.write().unwrap();
    reg.clear();
}
