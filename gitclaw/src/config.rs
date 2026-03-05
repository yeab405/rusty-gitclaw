use serde_yaml::Value;
use std::path::Path;
use tokio::fs;

#[derive(Debug, Clone, Default)]
pub struct EnvConfig {
    pub log_level: Option<String>,
    pub model_override: Option<String>,
    pub extra: serde_yaml::Value,
}

fn deep_merge(base: Value, overlay: Value) -> Value {
    match (base, overlay) {
        (Value::Mapping(mut base_map), Value::Mapping(overlay_map)) => {
            for (key, value) in overlay_map {
                let merged = if let Some(base_value) = base_map.remove(&key) {
                    deep_merge(base_value, value)
                } else {
                    value
                };
                base_map.insert(key, merged);
            }
            Value::Mapping(base_map)
        }
        (_, overlay) => overlay,
    }
}

async fn load_yaml_file(path: &Path) -> Value {
    match fs::read_to_string(path).await {
        Ok(raw) => serde_yaml::from_str(&raw).unwrap_or(Value::Mapping(serde_yaml::Mapping::new())),
        Err(_) => Value::Mapping(serde_yaml::Mapping::new()),
    }
}

pub async fn load_env_config(agent_dir: &Path, env: Option<&str>) -> EnvConfig {
    let config_dir = agent_dir.join("config");
    let env_name = env
        .map(String::from)
        .or_else(|| std::env::var("GITCLAW_ENV").ok());

    let base = load_yaml_file(&config_dir.join("default.yaml")).await;

    let merged = if let Some(env_name) = env_name {
        let env_override = load_yaml_file(&config_dir.join(format!("{env_name}.yaml"))).await;
        deep_merge(base, env_override)
    } else {
        base
    };

    let log_level = merged
        .get("log_level")
        .and_then(|v| v.as_str())
        .map(String::from);
    let model_override = merged
        .get("model_override")
        .and_then(|v| v.as_str())
        .map(String::from);

    EnvConfig {
        log_level,
        model_override,
        extra: merged,
    }
}
