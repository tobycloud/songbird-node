use core::f32;
use std::net::SocketAddr;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::sync::mpsc::UnboundedSender;
use axum::{
    extract::ws::{WebSocketUpgrade, WebSocket, Message},
    routing::get,
    response::{Response, IntoResponse},
    Router,
};
use async_trait::async_trait;
use songbird::{Driver, Config, ConnectionInfo, EventContext, id::{GuildId, UserId, ChannelId}, input::ffmpeg, Event, EventHandler, create_player};


/* 
#[tokio::main]
async fn main() {
    let app = Router::new()
    .route("/region", get(handler_region))
    .route("/voice", get(handler_ws));
    let server_addr = "127.0.0.1:8080";
    let addr_l: SocketAddr = server_addr.parse().expect("Unable to parse socket address");
    println!("listening on {}", addr_l.to_string());
    axum::Server::bind(&addr_l)
    .serve(app.into_make_service())
    .await
    .unwrap();
}
*/

#[shuttle_runtime::main]
async fn axum() -> ShuttleAxum {
    let app = Router::new()
    .route("/region", get(handler_region))
    .route("/voice", get(handler_ws));
    Ok(app.into())
}


async fn handler_region() -> Response {
    let mut body = reqwest::get("https://api.techniknews.net/ipgeo/").await.unwrap().text().await.unwrap().into_response();
    body.headers_mut().remove("Content-Type");
    body.headers_mut().append("Content-Type", "application/json".parse().unwrap());
    body
}

async fn handler_ws(ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(accept_connection)
}

struct Callback {
    ws: UnboundedSender<Message>,
    data: Value
}

#[async_trait]
impl EventHandler for Callback {
    async fn act(&self, _ctx: &EventContext<'_>) -> Option<Event> {
        self.ws.send(Message::Text(self.data.to_string())).unwrap();
        None
    }
}


async fn accept_connection(ws_stream: WebSocket) {
    let (mut write, mut read) = ws_stream.split();
    let (send_s, mut send_r) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
        loop {
            let read_data = send_r.recv().await;
            if read_data.is_none() { 
                write.close().await.unwrap();
                break;
            }
            let out = write.send(read_data.unwrap()).await;
            if out.is_err() { break; }
        }
    });
    let mut user_id = 0;
    let mut session_id = "".to_string();
    let mut channel_id= 0;
    let mut dr = Driver::new(Config::default());
    let jdata = json!({
        "t": "STOP"
    });
    let (mut _track, mut controler) = create_player(ffmpeg(" ").await.unwrap().into()); // make to stop panic when the control is already set when use
    dr.add_global_event(Event::Track(songbird::TrackEvent::End), Callback {ws: send_s.clone(), data: jdata});
    let mut volume = 100;
    while let Some(msg) = read.next().await {
        if msg.is_err() { 
            dr.leave();
            return; 
        }
        let msg = msg.unwrap();
        let msg = msg.to_text();
        if msg.is_ok() {
            let raw_o = serde_json::from_str(msg.unwrap());
            if raw_o.is_err() {
                drop(send_s.clone());
            }
            let out: serde_json::Value = raw_o.unwrap();
            let mut data_out = "";
            if out["t"].is_string() {
                data_out = out["t"].as_str().unwrap();
            }
            let data: serde_json::Value = out["d"].clone();
            if data_out == "VOICE_STATE_UPDATE" {
                let msg = data.as_object().unwrap();
                let sid = msg.get("session_id").unwrap().as_str().unwrap();
                session_id = sid.to_string();
                let uid = msg.get("user_id").unwrap().as_str().unwrap();
                user_id = uid.to_string().parse::<u64>().unwrap();
                let channel_id_raw = msg.get("channel_id").unwrap();
                if channel_id_raw.is_null() {
                    drop(send_s);
                    return;
                }
                channel_id = channel_id_raw.as_str().unwrap().to_string().parse::<u64>().unwrap();
            } else if data_out == "VOICE_SERVER_UPDATE" {
                let msg = data.as_object().unwrap();
                let token = msg.get("token").unwrap().as_str().unwrap().to_string();
                let guild_id = msg.get("guild_id").unwrap().as_str().unwrap().to_string().parse::<u64>().unwrap();
                let endpoint = msg.get("endpoint").unwrap().as_str().unwrap();
                dr.leave();
                dr.connect(ConnectionInfo {channel_id: Some(ChannelId(channel_id)), endpoint: endpoint.to_string(), guild_id: GuildId(guild_id), session_id: session_id.clone(), token, user_id: UserId(user_id)}).await.unwrap();
            } else if data_out == "PLAY" {
                let dataout = data.as_str().unwrap().to_string();
                dr.stop();
                let data = ffmpeg(dataout).await.unwrap();
                (_track, controler) = create_player(data);
                controler.set_volume(volume as f32 / 100.0).unwrap();
                dr.play(_track);
            } else if data_out == "VOLUME" {
                let dataout = data.as_i64().unwrap();
                volume = dataout;
                controler.set_volume(volume as f32 / 100.0).unwrap();
            } else if data_out == "PAUSE" {
                controler.pause().unwrap();
            } else if data_out == "RESUME" {
                controler.play().unwrap();
            } else if data_out == "STOP" {
                controler.stop().unwrap();
                dr.stop();
            } else if data_out == "PING" {
                let send_smg = json!({"t": "PONG"});
                let raw_json = Message::Text(send_smg.to_string());
                send_s.send(raw_json).unwrap();
            } 
        }
    }
}