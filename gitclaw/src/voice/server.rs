use axum::{routing::get, Json, Router};
use serde_json::json;

use super::adapter::{VoiceAdapter, VoiceServerOptions};
use super::openai_realtime::OpenAIRealtimeAdapter;
use crate::sdk::query;
use crate::sdk_types::QueryOptions;

fn dim(s: &str) -> String {
    format!("\x1b[2m{s}\x1b[0m")
}
fn bold(s: &str) -> String {
    format!("\x1b[1m{s}\x1b[0m")
}

/// Start the voice server: health check HTTP endpoint + OpenAI Realtime WebSocket.
/// Returns a cleanup closure.
pub async fn start_voice_server(
    opts: VoiceServerOptions,
) -> Result<impl FnOnce() -> futures::future::BoxFuture<'static, ()>, String> {
    let port = opts.port.unwrap_or(3333);

    // Create adapter
    let mut adapter = OpenAIRealtimeAdapter::new(opts.adapter_config);

    let agent_dir = opts.agent_dir.clone();
    let model = opts.model.clone();
    let env = opts.env.clone();

    // Tool handler: runs gitclaw query and collects response text
    let tool_handler: Box<
        dyn Fn(String) -> futures::future::BoxFuture<'static, String> + Send + Sync,
    > = Box::new(move |prompt: String| {
        let dir = agent_dir.clone();
        let model = model.clone();
        let env = env.clone();
        Box::pin(async move {
            let mut result = query(QueryOptions {
                prompt,
                dir: Some(dir),
                model,
                env,
                system_prompt: None,
                system_prompt_suffix: None,
                tools: None,
                replace_builtin_tools: false,
                allowed_tools: None,
                disallowed_tools: None,
                repo: None,
                max_turns: None,
                session_id: None,
                constraints: None,
            });

            let mut text = String::new();
            while let Some(msg) = result.next().await {
                if let crate::sdk_types::GCMessage::Assistant(ref a) = msg {
                    text.push_str(&a.content);
                }
            }

            if text.is_empty() {
                "(no response)".to_string()
            } else {
                text
            }
        })
    });

    // Start health check HTTP server
    let app = Router::new().route(
        "/health",
        get(|| async { Json(json!({ "status": "ok" })) }),
    );

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}"))
        .await
        .map_err(|e| format!("Failed to bind port {port}: {e}"))?;

    let server_handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    // Connect to OpenAI Realtime
    adapter.connect(tool_handler).await?;

    eprintln!(
        "{}",
        bold(&format!(
            "Voice server running on :{port} — connected to OpenAI Realtime"
        ))
    );

    // Return cleanup function
    Ok(move || -> futures::future::BoxFuture<'static, ()> {
        Box::pin(async move {
            server_handle.abort();
            eprintln!("{}", dim("[voice] Server stopped"));
        })
    })
}
