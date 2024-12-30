use serde_json::json;
use std::sync::Arc;
use tokio::sync::RwLock;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;

use crate::conn::{AetherWebRTCConnectionManager, ConnectionStatus};
use crate::rocket::futures::{SinkExt, StreamExt};

use tokio_tungstenite::connect_async;

use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::media_engine::MediaEngine;
use webrtc::api::APIBuilder;
use webrtc::interceptor::registry::Registry;

pub async fn start_server_connection(addr: &str, token: String) -> anyhow::Result<()> {
    let (ws_stream, _) = connect_async(&format!("{addr}/v1/landlord/ws?token={token}")).await?;

    let send_sync_ws_stream = Arc::new(RwLock::new(ws_stream));

    send_sync_ws_stream
        .write()
        .await
        .send(
            serde_json::json!({
                "type": "SPECIFICATION",
                    "message": {
                        "display": {
                            "width": 1920,
                            "height": 1080,
                            "frame_rate": 24,
                        },
                        "ip_addr": "0.0.0.0",
                        "device": {
                            "cpu": [
                                {
                                    "name": "<>",
                                    "size": 0,
                                }],
                            "gpu": [
                                {
                                    "name": "<>",
                                    "size": 0,
                            }]
                        }
                }
            })
            .to_string()
            .into(),
        )
        .await?;

    let mut engine = MediaEngine::default();

    engine
        .register_default_codecs()
        .expect("Unable to register default codecs.");

    let mut registry = Registry::new();

    registry = register_default_interceptors(registry, &mut engine)
        .expect("Unable to register default interceptors.");

    let api = APIBuilder::new()
        .with_media_engine(engine)
        .with_interceptor_registry(registry)
        .build();

    let (tx, mut rx) = tokio::sync::mpsc::channel(1);

    let state_ws = send_sync_ws_stream.clone();

    tokio::spawn(async move {
        tokio::select! {
            Some(state) = rx.recv() => {
                match state {
                    ConnectionStatus::Connected(uuid) => {
                        let _ = state_ws
                            .write()
                            .await
                            .send(
                                serde_json::json!({
                                    "type": "CONNECTION_MADE",
                                    "uuid": uuid
                                })
                                .to_string()
                                .into(),
                            )
                            .await;
                    }
                    ConnectionStatus::Disconnected(uuid) => {
                        let _ = state_ws
                            .write()
                            .await
                            .send(
                                serde_json::json!({
                                    "type": "DISCONNECTION_MADE",
                                    "uuid": uuid

                                })
                                .to_string()
                                .into(),
                            )
                            .await;
                    }
                    ConnectionStatus::ControlRelease(uuid) => {
                        let _ = state_ws
                            .write()
                            .await
                            .send(
                                serde_json::json!({
                                    "type": "CONTROL_RELEASED",
                                    "uuid": uuid
                                })
                                .to_string()
                                .into(),
                            )
                            .await;
                    }
                    ConnectionStatus::ControlTake(uuid) => {
                        let _ = state_ws
                            .write()
                            .await
                            .send(
                                serde_json::json!({
                                    "type": "CONTROL_TAKEN",
                                    "uuid": uuid
                                })
                                .to_string()
                                .into(),
                            )
                            .await;
                    }
                }
            }
        }
    });

    let mut conn_manager = AetherWebRTCConnectionManager::new(api, tx);

    while let Some(Ok(msg)) = send_sync_ws_stream.write().await.next().await {
        let data = serde_json::Value::from(msg.to_text()?);
        let request_type = data["type"].as_str();

        if request_type.is_none() {
            continue;
        }

        match request_type.unwrap() {
            "CONNECTION" => {
                if let Ok(answer) = conn_manager
                    .connect(
                        RTCSessionDescription::offer(data["sdp"].as_str().unwrap().into()).unwrap(),
                        data["uuid"].as_str().unwrap().into(),
                    )
                    .await
                {
                    let _ = send_sync_ws_stream
                        .write()
                        .await
                        .send(
                            json!({
                                "type": "CONNECTION_ACK",
                                "answer": answer
                            })
                            .to_string()
                            .into(),
                        )
                        .await;
                }
            }
            "CONTROL" => {
                let _ = conn_manager
                    .change_control_to(data["uuid"].to_string())
                    .await;

                let _ = send_sync_ws_stream
                    .write()
                    .await
                    .send(
                        json!({
                            "type": "CONTROL_ACK",
                            "uuid": data["uuid"]
                        })
                        .to_string()
                        .into(),
                    )
                    .await;
            }
            "DISCONNECT" => {
                let _ = conn_manager.disconnect_peer(data["uuid"].to_string()).await;

                let _ = send_sync_ws_stream
                    .write()
                    .await
                    .send(
                        json!({
                            "type": "DISCONNECT_ACK",
                            "uuid": data["uuid"]
                        })
                        .to_string()
                        .into(),
                    )
                    .await;
            }
            _ => {}
        }
    }

    todo!()
}
