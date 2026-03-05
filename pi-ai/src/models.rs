use once_cell::sync::Lazy;
use serde_json::Value;
use std::collections::HashMap;

use crate::types::{Model, Usage};

/// Raw JSON model registry, loaded once from embedded JSON.
static MODELS_JSON: &str = include_str!("../data/models.json");

/// Parsed model registry: provider -> model_id -> Model
static MODEL_REGISTRY: Lazy<HashMap<String, HashMap<String, Model>>> = Lazy::new(|| {
    let raw: HashMap<String, HashMap<String, Value>> =
        serde_json::from_str(MODELS_JSON).expect("Failed to parse models.json");

    let mut registry = HashMap::new();
    for (provider, models) in raw {
        let mut provider_models = HashMap::new();
        for (id, value) in models {
            // Remap JSON keys to Rust struct field names
            let model_value = remap_model_json(value);
            match serde_json::from_value::<Model>(model_value) {
                Ok(model) => {
                    provider_models.insert(id, model);
                }
                Err(e) => {
                    eprintln!("Warning: failed to parse model {provider}/{id}: {e}");
                }
            }
        }
        registry.insert(provider, provider_models);
    }
    registry
});

/// Remap camelCase JSON keys from the TS model registry to snake_case for serde.
fn remap_model_json(mut value: Value) -> Value {
    if let Some(obj) = value.as_object_mut() {
        // Rename keys
        if let Some(v) = obj.remove("baseUrl") {
            obj.insert("base_url".to_string(), v);
        }
        if let Some(v) = obj.remove("contextWindow") {
            obj.insert("context_window".to_string(), v);
        }
        if let Some(v) = obj.remove("maxTokens") {
            obj.insert("max_tokens".to_string(), v);
        }
        if let Some(cost) = obj.get_mut("cost") {
            if let Some(cost_obj) = cost.as_object_mut() {
                if let Some(v) = cost_obj.remove("cacheRead") {
                    cost_obj.insert("cache_read".to_string(), v);
                }
                if let Some(v) = cost_obj.remove("cacheWrite") {
                    cost_obj.insert("cache_write".to_string(), v);
                }
            }
        }
    }
    value
}

pub fn get_model(provider: &str, model_id: &str) -> Option<Model> {
    MODEL_REGISTRY
        .get(provider)
        .and_then(|models| models.get(model_id))
        .cloned()
}

pub fn get_providers() -> Vec<String> {
    MODEL_REGISTRY.keys().cloned().collect()
}

pub fn get_models(provider: &str) -> Vec<Model> {
    MODEL_REGISTRY
        .get(provider)
        .map(|models| models.values().cloned().collect())
        .unwrap_or_default()
}

pub fn calculate_cost(model: &Model, usage: &mut Usage) {
    usage.cost.input = (model.cost.input / 1_000_000.0) * usage.input as f64;
    usage.cost.output = (model.cost.output / 1_000_000.0) * usage.output as f64;
    usage.cost.cache_read = (model.cost.cache_read / 1_000_000.0) * usage.cache_read as f64;
    usage.cost.cache_write = (model.cost.cache_write / 1_000_000.0) * usage.cache_write as f64;
    usage.cost.total =
        usage.cost.input + usage.cost.output + usage.cost.cache_read + usage.cost.cache_write;
}

/// Check if a model supports xhigh thinking level.
pub fn supports_xhigh(model: &Model) -> bool {
    if model.id.contains("gpt-5.2") || model.id.contains("gpt-5.3") {
        return true;
    }
    if model.api == "anthropic-messages" {
        return model.id.contains("opus-4-6") || model.id.contains("opus-4.6");
    }
    false
}

pub fn models_are_equal(a: Option<&Model>, b: Option<&Model>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => a.id == b.id && a.provider == b.provider,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_model_registry_loads() {
        let providers = get_providers();
        assert!(!providers.is_empty());
        assert!(providers.contains(&"anthropic".to_string()));
        assert!(providers.contains(&"openai".to_string()));
    }

    #[test]
    fn test_get_model() {
        let model = get_model("anthropic", "claude-sonnet-4-5-20250929");
        assert!(model.is_some());
        let model = model.unwrap();
        assert_eq!(model.provider, "anthropic");
        assert_eq!(model.api, "anthropic-messages");
    }

    #[test]
    fn test_calculate_cost() {
        let model = get_model("anthropic", "claude-sonnet-4-5-20250929").unwrap();
        let mut usage = Usage {
            input: 1000,
            output: 500,
            ..Default::default()
        };
        calculate_cost(&model, &mut usage);
        assert!(usage.cost.total > 0.0);
    }
}
