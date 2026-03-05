use async_trait::async_trait;
use futures::SinkExt;
use serde_json::{json, Value};
use tokio::sync::Mutex;
use tokio_tungstenite::{connect_async, tungstenite::Message as WsMessage};

use super::adapter::{VoiceAdapter, VoiceAdapterConfig};

fn dim(s: &str) -> String {
    format!("\x1b[2m{s}\x1b[0m")
}

pub struct OpenAIRealtimeAdapter {
    config: VoiceAdapterConfig,
    ws_tx: Option<
        Mutex<
            futures::stream::SplitSink<
                tokio_tungstenite::WebSocketStream<
                    tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
                >,
                WsMessage,
            >,
        >,
    >,
}

impl OpenAIRealtimeAdapter {
    pub fn new(config: VoiceAdapterConfig) -> Self {
        Self {
            config,
            ws_tx: None,
        }
    }

    fn build_session_update(&self) -> Value {
        let instructions = self.config.instructions.clone().unwrap_or_else(|| {
            "You are a voice assistant for a git-based AI agent called gitclaw. \
             When the user asks you to do something with code, files, or their project, \
             use the run_agent tool to execute the request. Speak concisely."
                .to_string()
        });
        let voice = self.config.voice.as_deref().unwrap_or("alloy");

        json!({
            "type": "session.update",
            "session": {
                "instructions": instructions,
                "voice": voice,
                "turn_detection": { "type": "server_vad" },
                "input_audio_transcription": { "model": "whisper-1" },
                "tools": [{
                    "type": "function",
                    "name": "run_agent",
                    "description": "Run a gitclaw agent query to perform tasks like reading files, writing code, running commands, etc.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "query": {
                                "type": "string",
                                "description": "The user's request to pass to the gitclaw agent"
                            }
                        },
                        "required": ["query"]
                    }
                }]
            }
        })
    }
}

#[async_trait]
impl VoiceAdapter for OpenAIRealtimeAdapter {
    async fn connect(
        &mut self,
        tool_handler: Box<
            dyn Fn(String) -> futures::future::BoxFuture<'static, String> + Send + Sync,
        >,
    ) -> Result<(), String> {
        let model = self
            .config
            .model
            .as_deref()
            .unwrap_or("gpt-4o-realtime-preview");
        let url = format!("wss://api.openai.com/v1/realtime?model={model}");

        let request = http::Request::builder()
            .uri(&url)
            .header("Authorization", format!("Bearer {}", self.config.api_key))
            .header("OpenAI-Beta", "realtime=v1")
            .header("Host", "api.openai.com")
            .header("Connection", "Upgrade")
            .header("Upgrade", "websocket")
            .header("Sec-WebSocket-Version", "13")
            .header(
                "Sec-WebSocket-Key",
                tokio_tungstenite::tungstenite::handshake::client::generate_key(),
            )
            .body(())
            .map_err(|e| format!("Failed to build request: {e}"))?;

        let (ws_stream, _) = connect_async(request)
            .await
            .map_err(|e| format!("WebSocket connection failed: {e}"))?;

        let (write, mut read) = futures::StreamExt::split(ws_stream);
        let write = Mutex::new(write);

        // Send session update
        let session_update = self.build_session_update();
        {
            let mut w = write.lock().await;
            w.send(WsMessage::Text(
                serde_json::to_string(&session_update).unwrap(),
            ))
            .await
            .map_err(|e| format!("Failed to send session update: {e}"))?;
        }

        let tool_handler = std::sync::Arc::new(tool_handler);

        // Spawn message handler
        let write_clone = std::sync::Arc::new(write);
        let write_for_handler = write_clone.clone();

        tokio::spawn(async move {
            use futures::StreamExt;
            while let Some(msg) = read.next().await {
                let msg = match msg {
                    Ok(m) => m,
                    Err(e) => {
                        eprintln!("{}", dim(&format!("[voice] WebSocket error: {e}")));
                        break;
                    }
                };

                if let WsMessage::Text(text) = msg {
                    let event: Value = match serde_json::from_str(&text) {
                        Ok(v) => v,
                        Err(_) => continue,
                    };

                    let event_type = event["type"].as_str().unwrap_or("");

                    match event_type {
                        "session.created" => {
                            eprintln!("{}", dim("[voice] Session created"));
                        }
                        "session.updated" => {
                            eprintln!("{}", dim("[voice] Session configured"));
                        }
                        "conversation.item.input_audio_transcription.completed" => {
                            if let Some(transcript) = event["transcript"].as_str() {
                                eprintln!("{}", dim(&format!("[voice] User: {transcript}")));
                            }
                        }
                        "response.function_call_arguments.done" => {
                            let call_id =
                                event["call_id"].as_str().unwrap_or("").to_string();
                            let name = event["name"].as_str().unwrap_or("");

                            if name != "run_agent" {
                                eprintln!(
                                    "{}",
                                    dim(&format!("[voice] Unknown function call: {name}"))
                                );
                                continue;
                            }

                            let args_str =
                                event["arguments"].as_str().unwrap_or("{}");
                            let args: Value =
                                serde_json::from_str(args_str).unwrap_or(json!({}));
                            let query = args["query"]
                                .as_str()
                                .unwrap_or("")
                                .to_string();

                            eprintln!(
                                "{}",
                                dim(&format!("[voice] Agent query: {query}"))
                            );

                            let handler = tool_handler.clone();
                            let write_ref = write_for_handler.clone();
                            let cid = call_id.clone();

                            tokio::spawn(async move {
                                let result = handler(query).await;

                                let preview = if result.len() > 200 {
                                    format!("{}...", &result[..200])
                                } else {
                                    result.clone()
                                };
                                eprintln!(
                                    "{}",
                                    dim(&format!("[voice] Agent response: {preview}"))
                                );

                                let output_event = json!({
                                    "type": "conversation.item.create",
                                    "item": {
                                        "type": "function_call_output",
                                        "call_id": cid,
                                        "output": result,
                                    }
                                });

                                let mut w = write_ref.lock().await;
                                let _ = w
                                    .send(WsMessage::Text(
                                        serde_json::to_string(&output_event).unwrap(),
                                    ))
                                    .await;
                                let _ = w
                                    .send(WsMessage::Text(
                                        json!({"type": "response.create"}).to_string(),
                                    ))
                                    .await;
                            });
                        }
                        "error" => {
                            eprintln!(
                                "{}",
                                dim(&format!(
                                    "[voice] Error: {}",
                                    serde_json::to_string(&event["error"])
                                        .unwrap_or_default()
                                ))
                            );
                        }
                        _ => {}
                    }
                }
            }
            eprintln!("{}", dim("[voice] WebSocket closed"));
        });

        // Store write half (not needed for disconnect in this design, but kept for API)
        // self.ws_tx is not easily storable due to Arc<Mutex<...>>
        // The connection lives in the spawned task

        Ok(())
    }

    async fn disconnect(&mut self) -> Result<(), String> {
        // The WebSocket will be closed when the task is dropped
        // In a production implementation, we'd store the task handle and cancel it
        Ok(())
    }
}
