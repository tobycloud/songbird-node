#![allow(dead_code)]
mod ffmpeg_support;
mod youtube_supprt;
mod task_support;

use core::f32;
use std::{net::SocketAddr, num::NonZeroU64, sync::atomic::AtomicBool, io::Cursor};
use ffmpeg_support::get_input;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use sysinfo::{System, SystemExt, Pid, ProcessExt};
use tokio::sync::mpsc::UnboundedSender;
use axum::{
    extract::ws::{WebSocketUpgrade, WebSocket, Message},
    routing::get,
    response::{Response, IntoResponse},
    Router, http,
    extract::Request,
    http::StatusCode,
    middleware::{Next, from_fn},
};
use async_trait::async_trait;
use songbird::{Driver, Config, ConnectionInfo, EventContext, id::{GuildId, ChannelId}, Event, EventHandler, tracks::TrackHandle, model::{payload::Speaking, id::UserId}, CoreEvent, driver::DecodeMode};
use serde::{Deserialize, Serialize};
use songbird::id::UserId as RawUserId;
use json_comments::StripComments;
use lazy_static::lazy_static;
use youtube_supprt::youtube_modun;
use tokio::sync::Mutex;
use dashmap::DashMap;
use chrono::{DateTime, Utc};
use base64::Engine as _;

lazy_static! {
    static ref ROOT_CONFIG: ConfigFile = {
        let file_data = std::fs::read("config.json").unwrap();
        let stripped = StripComments::new(file_data.as_slice());
        let root_config: ConfigFile = serde_json::from_reader(stripped).expect("Config Error: Couldn't parse config");
        root_config
    };
    static ref REGION: Mutex<String> = {
        Mutex::new("".to_string())
    };
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ConfigFile {
    pub bind: String,
    pub auth: Value,
}

async fn auth(req: Request, next: Next) -> Result<Response, StatusCode> {
    if req.uri().path() == "/" { return Err(StatusCode::OK); }
    if ROOT_CONFIG.auth.is_string() {            
        let auth_header = req.headers().get(http::header::AUTHORIZATION).and_then(|header| header.to_str().ok());
        if auth_header.is_none() { return Err(StatusCode::UNAUTHORIZED); }
        else if auth_header.unwrap() != ROOT_CONFIG.auth.as_str().unwrap() { return Err(StatusCode::UNAUTHORIZED); }
    } else if !ROOT_CONFIG.auth.is_null() && !ROOT_CONFIG.auth.is_string() {
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }
    Ok(next.run(req).await)
}

#[tokio::main]
async fn main() {
    *REGION.lock().await = reqwest::get("https://api.techniknews.net/ipgeo/").await.unwrap().text().await.unwrap();
    println!("get region successfully");
    let app = Router::new()
    .route("/", get(handler_root))
    .route("/region", get(handler_region))
    .route("/status", get(handler_status))
    .route("/voice", get(handler_ws))
    .layer(from_fn(auth));
    let server_addr = &ROOT_CONFIG.bind;
    let addr_l: SocketAddr = server_addr.parse().expect("Unable to parse socket address");
    println!("listening on {}", addr_l.to_string());
    let listener = tokio::net::TcpListener::bind(addr_l)
        .await
        .unwrap();
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
    .unwrap();
}

async fn handler_root() -> StatusCode {
    StatusCode::OK
}

async fn handler_status() -> Response {
    let youtube_status = !reqwest::get("https://manifest.googlevideo.com/api/manifest/hls_playlist/").await.unwrap().status().eq(&reqwest::StatusCode::TOO_MANY_REQUESTS);
    let a = tokio::task::spawn_blocking(move || {
        let pid = Pid::from(std::process::id() as usize);
        let mut sys = System::new();
        sys.refresh_all();
        let mut player_cout = 0;
        let pros = sys.process(pid).unwrap();
        for i in sys.processes_by_name("ffmpeg") {
            if i.parent().is_some_and(|x| x == pros.pid()) {
                player_cout += 1;
            }
        }
        if !youtube_status {
            player_cout += 1e99 as i32;
        }
        let out = json!({
                                "players": player_cout,
            });
        out
    }).await.unwrap();
    let mut res = a.to_string().into_response();
    res.headers_mut().remove("Content-Type");
    res.headers_mut().append("Content-Type", "application/json".parse().unwrap());
    res
}


async fn handler_region() -> Response {
    let mut body = REGION.lock().await.clone().into_response();
    body.headers_mut().remove("Content-Type");
    body.headers_mut().append("Content-Type", "application/json".parse().unwrap());
    body
}

async fn handler_ws(ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(accept_connection)
}

fn to_wav(pcm_samples: &[i16], buffer: &mut Vec<u8>) -> Result<(), hound::Error> {
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: 48000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };

    let cursor = Cursor::new(buffer);

    let mut writer = hound::WavWriter::new(cursor, spec)?;

    for &sample in pcm_samples {
        writer.write_sample(sample)?;
    }

    Ok(())
}

#[derive(Clone)]
struct Callback {
    ws: UnboundedSender<Message>,
    data: Value,
    data_err: Value,
}

#[async_trait]
impl EventHandler for Callback {
    async fn act(&self, ctx: &EventContext<'_>) -> Option<Event> {
        match ctx {
            EventContext::Track(ts_raw) => {
                let ts = ts_raw.get(0).unwrap();
                let data;
                match ts.0.playing {
                    songbird::tracks::PlayMode::Stop => {
                        if ts.0.playing.is_done() {
                            data = self.data.to_string();
                        } else {
                            data = "".to_string();
                        }
                    },
                    songbird::tracks::PlayMode::End => {
                        if ts.0.playing.is_done() {
                            data = self.data.to_string();
                        } else {
                            data = "".to_string();
                        }
                    },
                    songbird::tracks::PlayMode::Errored(_) => {
                        data = self.data_err.to_string();
                    },
                    _ => todo!(),   
                }
                if !data.is_empty() {
                    self.ws.send(Message::Text(data)).unwrap();
                }
            },
            _ => return None,
        }
        None
    }
}



#[derive(Clone, Debug)]
struct Snippet {
    date: DateTime<Utc>,
    mapping: Option<UserInfo>,
}

#[derive(PartialEq, Copy, Clone, Debug)]
struct UserInfo {
    user_id: u64
}

struct InnerReceiver {
    last_tick_was_empty: AtomicBool,
    known_ssrcs: DashMap<u32, UserId>,
}

#[derive(Clone)]
struct CallbackR {
    ws: UnboundedSender<Message>
}


impl CallbackR {
    pub fn new(ws: UnboundedSender<Message>) -> Self {
        // You can manage state here, such as a buffer of audio packet bytes so
        // you can later store them in intervals.
        Self {
            ws
        }
    }
}

#[async_trait]
impl EventHandler for CallbackR {
    #[allow(unused_variables)]
    async fn act(&self, ctx: &EventContext<'_>) -> Option<Event> {
        use EventContext as Ctx;
        match ctx {
            Ctx::SpeakingStateUpdate(Speaking {
                speaking,
                ssrc,
                user_id,
                ..
            }) => {

                if user_id.is_none() {
                    return None;
                }
                let jdata = json!({
                    "t": "SSRC_UPDATE",
                    "d": {
                        "ssrc": ssrc,
                        "user": user_id.unwrap().0
                    }
                });
                self.ws.send(Message::Text(jdata.to_string())).unwrap();
                
            },
            Ctx::VoiceTick(packet) => {
                for i in &packet.speaking {
                    let data_out = &i.1.decoded_voice;
                    if data_out.is_some() {
                        let data = data_out.as_ref().unwrap();
                        let mut data_u8 = Vec::new();
                        to_wav(data, &mut data_u8).unwrap();
                        let b64_data = base64::engine::general_purpose::URL_SAFE.encode(data_u8);
                        let jdata = json!({
                            "t": "VOICE_PACKET",
                            "d": {
                                "ssrc": i.0,
                                "data": b64_data
                            }
                        });
                        self.ws.send(Message::Text(jdata.to_string())).unwrap();
                    }
                }
            },
            Ctx::DriverConnect(data) => {
                let jdata = json!({
                    "t": "CONNECTED"
                });
                let _ = self.ws.send(Message::Text(jdata.to_string()));
            },
            _ => {
                // We won't be registering this struct for any more event classes.
                unimplemented!()
            },
        }
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
    let mut user_id = 1;
    let mut session_id = "".to_string();
    let mut channel_id = 1;
    let mut dr = Driver::new(Config::default().decode_mode(DecodeMode::Decode));
    let jdata = json!({
        "t": "STOP"
    });
    let jdata_err = json!({
        "t": "STOP_ERROR"
    });
    let mut controler: Option<TrackHandle> = None;
    let evt_receiver = CallbackR::new(send_s.clone());
    let track_event = Callback {ws: send_s.clone(), data: jdata.clone(), data_err: jdata_err.clone()};
    dr.add_global_event(Event::Track(songbird::TrackEvent::End), track_event.clone());
    dr.add_global_event(Event::Track(songbird::TrackEvent::Error), track_event.clone());
    dr.add_global_event(CoreEvent::SpeakingStateUpdate.into(), evt_receiver.clone());
    dr.add_global_event(CoreEvent::VoiceTick.into(), evt_receiver.clone());
    dr.add_global_event(CoreEvent::ClientDisconnect.into(), evt_receiver.clone());
    dr.add_global_event(CoreEvent::DriverConnect.into(), evt_receiver.clone());
    let mut volume = 100;
    while let Some(msg) = read.next().await {
        if msg.is_err() { 
            dr.leave();
            return; 
        }
        let msg = msg.unwrap();
        let msg = msg.to_text();
        if msg.is_ok() {
            let uq = msg.unwrap();
            if uq.is_empty() {
                drop(send_s.clone());
                return;
            }
            let raw_o = serde_json::from_str(uq);
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
                    dr.leave();
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
                dr.connect(ConnectionInfo {channel_id: Some(ChannelId(NonZeroU64::new(channel_id).unwrap())), endpoint: endpoint.to_string(), guild_id: GuildId(NonZeroU64::new(guild_id).unwrap()), session_id: session_id.clone(), token, user_id: RawUserId(NonZeroU64::new(user_id).unwrap())}).await.unwrap();
            } else if data_out == "PLAY" {
                let dataout = data["url"].as_str().unwrap().to_string();
                let stop_op = controler.as_mut();
                if stop_op.is_some() {
                    let _ = stop_op.unwrap().stop();
                }
                dr.stop();
                let data_input;
                if data["type"].is_string() {
                    let jdata_err = json!({
                        "t": "STOP_ERROR"
                    });
                    if data["type"].as_str().unwrap() == "youtube" {
                        let data_input_raw = youtube_modun(dataout).await;
                        if data_input_raw.is_err() { 
                            let _ = send_s.send(Message::Text(jdata_err.to_string())); 
                            continue;
                        }
                        data_input = data_input_raw.unwrap();
                    } else {
                        let _ = send_s.send(Message::Text(jdata_err.to_string()));
                        continue;
                    }
                } else {
                    data_input = get_input(dataout).await; 
                }
                controler = Some(dr.play_input(data_input));
                let _ = controler.as_mut().unwrap().set_volume(volume as f32 / 100.0);
            } else if data_out == "VOLUME" {
                let dataout = data.as_i64().unwrap();
                volume = dataout;
                let _ = controler.as_mut().unwrap().set_volume(volume as f32 / 100.0);
            } else if data_out == "PAUSE" {
                let _ = controler.as_mut().unwrap().pause();
            } else if data_out == "RESUME" {
                let _ = controler.as_mut().unwrap().play();
            } else if data_out == "STOP" {
                let _ = controler.as_mut().as_mut().unwrap().stop();
                dr.stop();
            } else if data_out == "PING" {
                let send_smg = json!({"t": "PONG"});
                let raw_json = Message::Text(send_smg.to_string());
                send_s.send(raw_json).unwrap();
            } 
        }
    }
}