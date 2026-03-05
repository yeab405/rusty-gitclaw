use async_trait::async_trait;

#[derive(Debug, Clone)]
pub struct VoiceAdapterConfig {
    pub api_key: String,
    pub model: Option<String>,
    pub voice: Option<String>,
    pub instructions: Option<String>,
}

#[async_trait]
pub trait VoiceAdapter: Send + Sync {
    async fn connect(
        &mut self,
        tool_handler: Box<dyn Fn(String) -> futures::future::BoxFuture<'static, String> + Send + Sync>,
    ) -> Result<(), String>;
    async fn disconnect(&mut self) -> Result<(), String>;
}

pub struct VoiceServerOptions {
    pub port: Option<u16>,
    pub adapter: String,
    pub adapter_config: VoiceAdapterConfig,
    pub agent_dir: String,
    pub model: Option<String>,
    pub env: Option<String>,
}
