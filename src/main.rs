use core::f32;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::sync::mpsc::UnboundedSender;
use tokio_tungstenite::tungstenite::Message;
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::{
    accept_hdr_async,
    tungstenite::handshake::server::{Request, Response},
};
use async_trait::async_trait;
use songbird::{Driver, Config, ConnectionInfo, EventContext, id::{GuildId, UserId, ChannelId}, input::Restartable, Event, EventHandler, create_player};

#[tokio::main]
async fn main() {
    let server = TcpListener::bind("127.0.0.1:8080").await.unwrap();

    while let Ok((stream, _)) = server.accept().await {
        tokio::spawn(accept_connection(stream));
    }
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

async fn accept_connection(stream: TcpStream) {
    let callback = |_req: &Request, response: Response| {
        /*
        let key = "test";
        let auth_op = req.headers().get("Authorization");
         if auth_op.is_none() {
            let response = Response::builder()
            .status(StatusCode::FORBIDDEN)
            .body(None).unwrap();
            return Err(response);
        } else if auth_op.unwrap() != key {
            let response = Response::builder()
            .status(StatusCode::FORBIDDEN)
            .body(None).unwrap();
            return Err(response);
        } */
        Ok(response)
    };
    let (send_s, mut send_r) = tokio::sync::mpsc::unbounded_channel();
    let ws_stream = accept_hdr_async(stream, callback)
        .await
        .expect("Error during the websocket handshake occurred");
    let (mut write, mut read) = ws_stream.split();
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
    let (mut _track, mut controler) = create_player(Restartable::ffmpeg(" ", false).await.unwrap().into()); // make to stop panic when the control is already set when use
    dr.add_global_event(Event::Track(songbird::TrackEvent::End), Callback {ws: send_s.clone(), data: jdata});
    let mut volume = 100;
    while let Some(msg) = read.next().await {
        if msg.is_err() { 
            dr.leave();
            return; 
        }
        let msg = msg.unwrap();
        if msg.is_text() {
            let out: serde_json::Value = serde_json::from_str(msg.to_text().unwrap()).unwrap();
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
                print!("{:?}\n\n", msg);
            } else if data_out == "VOICE_SERVER_UPDATE" {
                let msg = data.as_object().unwrap();
                let token = msg.get("token").unwrap().as_str().unwrap().to_string();
                let guild_id = msg.get("guild_id").unwrap().as_str().unwrap().to_string().parse::<u64>().unwrap();
                let endpoint = msg.get("endpoint").unwrap().as_str().unwrap();
                dr.leave();
                dr.connect(ConnectionInfo {channel_id: Some(ChannelId(channel_id)), endpoint: endpoint.to_string(), guild_id: GuildId(guild_id), session_id: session_id.clone(), token, user_id: UserId(user_id)}).await.unwrap();
            } else if data_out == "PLAY" {
                let dataout = data.as_str().unwrap().to_string();
                controler.stop().unwrap();
                dr.stop();
                let data = Restartable::ffmpeg(dataout, false).await.unwrap();
                (_track, controler) = create_player(data.into());
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