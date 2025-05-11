use std::sync::Arc;

use crate::types::AppState;
use axum::{
    extract::{ws::WebSocket, Json, State, WebSocketUpgrade},
    response::Response,
    routing::{any, get, post},
    Router,
};
use futures_util::{lock::Mutex, SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::json;

pub fn get_router() -> Router {
    let state = Arc::new(Mutex::new(AppState::new()));
    Router::new()
        .route("/", get(handler_root))
        .route("/add_token", post(add_token))
        .route("/voice", any(handler_voice))
        .with_state(state)
}

async fn handler_root() -> &'static str {
    "Hello, World!"
}

#[derive(Deserialize)]
struct AddToken {
    token: Vec<String>,
}

async fn add_token(
    State(state): State<Arc<Mutex<AppState>>>,
    Json(payload): Json<AddToken>,
) -> String {
    let mut state = state.lock().await;
    for token in payload.token.clone() {
        state.add_token(token).await
    }
    format!("Added {} tokens", payload.token.len())
}

async fn handler_voice(
    State(state): State<Arc<Mutex<AppState>>>,
    ws: WebSocketUpgrade,
) -> Response {
    println!(
        "New voice socket connected, number of clients: {}",
        state.clone().lock().await.discord_clients.len()
    );
    ws.on_upgrade(move |socket| handle_voice_socket(socket, state.clone()))
}

fn json_value_to_axum_message(json_value: serde_json::Value) -> axum::extract::ws::Message {
    axum::extract::ws::Message::Text(json_value.to_string())
}

async fn handle_voice_socket(socket: WebSocket, state: Arc<Mutex<AppState>>) {
    let (send_ws, recv_ws) = async_channel::unbounded::<axum::extract::ws::Message>();
    let (mut write, mut read) = socket.split();

    tokio::spawn(async move {
        while let Ok(msg) = recv_ws.recv().await {
            if let Err(e) = write.send(msg).await {
                //client disconnected
                println!("Error sending message: {}", e);
            }
        }
    });
    let mut guild_id = "".to_string();
    let mut channel_id = "".to_string();
    while let Some(msg) = read.next().await {
        let msg = if let Ok(msg) = msg {
            msg
        } else {
            // client disconnected
            if (guild_id.clone().len() > 0) {
                state.lock().await.leave_voice(guild_id).await;
            }
            return;
        };

        match msg {
            axum::extract::ws::Message::Text(text) => {
                let command = {
                    if let Ok(command) = serde_json::from_str::<serde_json::Value>(&text) {
                        command
                    } else {
                        send_ws
                            .send(json_value_to_axum_message(json!({
                                "error": "Invalid command",
                                "op":4000
                            })))
                            .await
                            .unwrap();
                        continue;
                    }
                };
                let t = command["t"].as_str().unwrap();
                match t {
                    "join_voice" => {
                        let gid = command["guild_id"].as_str().unwrap().to_string();
                        let cid = command["channel_id"].as_str().unwrap().to_string();
                        // let user_id = command["user_id"].as_str().unwrap().to_string();
                        guild_id = gid.clone();
                        channel_id = cid.clone();
                        (match state
                            .lock()
                            .await
                            .join_voice(gid.clone(), cid.clone())
                            .await
                        {
                            Ok(sender) => {
                                let send_ws = send_ws.clone();
                                tokio::spawn(async move {
                                    while let Ok(msg) = sender.recv().await {
                                        send_ws.send(msg).await.unwrap();
                                    }
                                });
                            }
                            Err(e) => {
                                send_ws
                                    .send(json_value_to_axum_message(json!({
                                        "error": e,
                                        "op":4000
                                    })))
                                    .await
                                    .unwrap();
                                return;
                            }
                        });
                    }
                    "leave_voice" => {
                        let gid = command["guild_id"].as_str().unwrap().to_string();
                        guild_id = "".to_string();
                        state.lock().await.leave_voice(gid).await
                    }
                    "play" => {
                        let guild_id = command["guild_id"].as_str().unwrap().to_string();
                        let url = command["url"].as_str().unwrap().to_string();
                        state
                            .lock()
                            .await
                            .play(guild_id, channel_id.clone(), url)
                            .await
                    }
                    "add_tokens" =>{
                        let token = command["token"].as_array().unwrap();
                        for t in token {
                            state.lock().await.add_token(t.as_str().unwrap().to_string()).await;
                        }
                        send_ws.send(json_value_to_axum_message(json!({"op": 4001}))).await.unwrap();
                    }
                    _ => {
                        send_ws
                            .send(json_value_to_axum_message(json!({
                                "error": "Invalid command",
                                "op":4000
                            })))
                            .await
                            .unwrap();
                    }
                }
            }
            axum::extract::ws::Message::Binary(_) => {
                send_ws
                    .send(axum::extract::ws::Message::Binary(
                        "We currently don't support binary messages"
                            .as_bytes()
                            .to_vec(),
                    ))
                    .await
                    .unwrap();
            }
            axum::extract::ws::Message::Ping(items) => {
                send_ws
                    .send(axum::extract::ws::Message::Pong(items))
                    .await
                    .unwrap();
            }
            axum::extract::ws::Message::Pong(items) => send_ws
                .send(axum::extract::ws::Message::Pong(items))
                .await
                .unwrap(),
            axum::extract::ws::Message::Close(close_frame) => {
                send_ws
                    .send(axum::extract::ws::Message::Close(close_frame))
                    .await
                    .unwrap();
            }
        }
    }
}
