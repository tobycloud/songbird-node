use std::collections::{HashMap, HashSet};
use std::num::NonZeroU64;
use std::sync::Arc;
use std::time::Duration;

use async_channel::{unbounded, Receiver, Sender};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use songbird::{
    driver::DecodeMode,
    id::{ChannelId, GuildId, UserId as RawUserId},
    tracks::TrackHandle,
    Config, ConnectionInfo, CoreEvent, Driver, Event,
};
use tokio::sync::Mutex;
use tokio_tungstenite::{connect_async, tungstenite::Message};

use crate::ffmpeg_support::get_input;

#[derive(Clone)]
pub struct Guild {
    id: String,
    name: String,
    owner_id: String,
    raw: Value,
}

impl Guild {
    pub fn new(raw: Value) -> Result<Self, String> {
        let id = raw["id"]
            .as_str()
            .ok_or_else(|| "Missing guild ID".to_string())?;
        let name = raw["name"]
            .as_str()
            .ok_or_else(|| "Missing guild name".to_string())?;
        let owner_id = raw["owner_id"]
            .as_str()
            .ok_or_else(|| "Missing guild owner ID".to_string())?;

        Ok(Self {
            id: id.to_string(),
            name: name.to_string(),
            owner_id: owner_id.to_string(),
            raw,
        })
    }
}

#[derive(Clone, Debug)]
pub struct User {
    raw: Value,
    pub id: String,
    username: String,
}

impl User {
    pub fn new(raw: Value) -> Result<Self, String> {
        let id = raw["id"]
            .as_str()
            .ok_or_else(|| "Missing user ID".to_string())?;
        let username = raw["username"]
            .as_str()
            .ok_or_else(|| "Missing username".to_string())?;

        Ok(Self {
            id: id.to_string(),
            username: username.to_string(),
            raw: raw.clone(),
        })
    }
}

#[derive(Clone)]
struct Connection {
    driver: Arc<Mutex<Driver>>,
    controler: Option<TrackHandle>,
    volume: f32,
    sender: Sender<axum::extract::ws::Message>,
    guild_id: String,
    channel_id: String,
}

impl Connection {
    async fn setup_driver_event(
        dr: Arc<Mutex<Driver>>,
        send_s: Sender<axum::extract::ws::Message>,
    ) {
        let evt_receiver = crate::obj_event::CallbackR::new(send_s.clone());
        let track_event = crate::obj_event::Callback { ws: send_s.clone() };
        let mut dr = dr.lock().await;
        dr.add_global_event(
            Event::Track(songbird::TrackEvent::Play),
            track_event.clone(),
        );
        dr.add_global_event(
            Event::Track(songbird::TrackEvent::Pause),
            track_event.clone(),
        );
        dr.add_global_event(Event::Track(songbird::TrackEvent::End), track_event.clone());
        dr.add_global_event(
            Event::Track(songbird::TrackEvent::Error),
            track_event.clone(),
        );
        dr.add_global_event(CoreEvent::SpeakingStateUpdate.into(), evt_receiver.clone());
        dr.add_global_event(CoreEvent::VoiceTick.into(), evt_receiver.clone());
        dr.add_global_event(CoreEvent::DriverConnect.into(), evt_receiver.clone());
    }

    pub async fn new(
        sender: Sender<axum::extract::ws::Message>,
        guild_id: String,
        channel_id: String,
    ) -> Self {
        let dr = Arc::new(Mutex::new(Driver::new(
            Config::default().decode_mode(DecodeMode::Decode),
        )));
        Self::setup_driver_event(dr.clone(), sender.clone()).await;
        Self {
            driver: dr.clone(),
            controler: None,
            volume: 100.0,
            sender,
            guild_id,
            channel_id,
        }
    }
    pub async fn play(&mut self, url: String) {
        let stop_op = self.controler.as_mut();
        if stop_op.is_some() {
            let _ = stop_op.unwrap().stop();
        }
        let data_input = get_input(url).await;
        self.controler = Some(self.driver.lock().await.play_input(data_input));
        let _ = self
            .controler
            .as_mut()
            .unwrap()
            .set_volume(self.volume as f32 / 100.0);
    }

    pub async fn connect(
        &mut self,
        connection_info: ConnectionInfo,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut driver = self.driver.lock().await;
        driver.connect(connection_info).await?;
        self.sender
            .send(axum::extract::ws::Message::Text(
                json!({
                    "op": 4042,
                    "t": "JOIN_VOICE_CHANNEL",
                    "d": {
                        "guild_id": self.guild_id,
                        "channel_id": self.channel_id,
                    }
                })
                .to_string(),
            ))
            .await?;
        Ok(())
    }
    pub async fn pause(&mut self) {
        if let Some(controler) = self.controler.as_mut() {
            let _ = controler.pause();
        }
    }

    pub async fn resume(&mut self) {
        if let Some(controler) = self.controler.as_mut() {
            let _ = controler.play();
        }
    }

    pub async fn stop(&mut self) {
        if let Some(controler) = self.controler.as_mut() {
            let _ = controler.stop();
        }
    }

    pub async fn volume(&mut self, volume: f32) {
        if let Some(controler) = self.controler.as_mut() {
            let _ = controler.set_volume(volume as f32 / 100.0);
        }
        self.volume = volume;
    }
}

#[derive(Clone)]
pub struct VoiceState {
    guild_id: String,
    channel_id: Value,
    user_id: String,
    session_id: String,
    mute: bool,
    deaf: bool,
    self_mute: bool,
    self_deaf: bool,
}

impl VoiceState {
    pub fn new(json_message: Value) -> Self {
        Self {
            guild_id: json_message["guild_id"].as_str().unwrap().to_string(),
            channel_id: json_message["channel_id"].clone(),
            user_id: json_message["user_id"].as_str().unwrap().to_string(),
            session_id: json_message["session_id"].as_str().unwrap().to_string(),
            mute: json_message["mute"].as_bool().unwrap(),
            deaf: json_message["deaf"].as_bool().unwrap(),
            self_mute: json_message["self_mute"].as_bool().unwrap(),
            self_deaf: json_message["self_deaf"].as_bool().unwrap(),
        }
    }
}

#[derive(Clone)]
pub struct DiscordClient {
    token: String,
    ws_stream_tx: Sender<Message>,
    ws_stream_rx: Receiver<Message>,
    // sender message for the websocket
    sender: Sender<Message>,
    // receiver message for the websocket
    receiver: Receiver<Message>,
    pub is_connected: bool,
    heartbeat_interval: u64,

    guilds: HashMap<String, Guild>,
    pub user: Option<User>,
    session_id: String,
    guilds_ids: HashSet<String>,
    resume_gateway_url: String,
    driver: HashMap<String, Connection>,
    joined_guilds: HashSet<String>,
    voice_states: HashMap<String, HashMap<String, VoiceState>>,

    event_sender: Sender<axum::extract::ws::Message>,
}

impl DiscordClient {
    pub fn new(token: String, event_sender: Sender<axum::extract::ws::Message>) -> Self {
        let (tx_sender, rx_sender) = unbounded();
        let (tx_receiver, rx_receiver) = unbounded();
        Self {
            token,
            ws_stream_tx: tx_sender,
            ws_stream_rx: rx_receiver,
            sender: tx_receiver,
            receiver: rx_sender,
            is_connected: false,
            heartbeat_interval: 0,
            guilds: HashMap::new(),
            user: None,
            session_id: "".to_string(),
            guilds_ids: HashSet::new(),
            resume_gateway_url: "".to_string(),
            driver: HashMap::new(),
            joined_guilds: HashSet::new(),
            voice_states: HashMap::new(),
            event_sender,
        }
    }

    pub async fn connect(&mut self) {
        let url = format!("wss://gateway.discord.gg/?v=10&encoding=json");
        let (ws_stream, _) = connect_async(url).await.unwrap();
        let (mut write, mut read) = ws_stream.split();
        let tx = self.ws_stream_tx.clone();
        let rx = self.ws_stream_rx.clone();
        // read the messages from the websocket and send the message in the channel if the message is a text message
        tokio::spawn(async move {
            while let Some(Ok(message)) = read.next().await {
                tx.send(message).await.unwrap();
            }
        });
        // read the messages from the channel and send them to the websocket
        tokio::spawn(async move {
            while let Ok(message) = rx.recv().await {
                write.send(message).await.unwrap();
            }
        });

        let mut client = self.clone();
        tokio::spawn(async move {
            client.handle_ws().await;
        });
    }

    pub async fn check_voice_channels(&mut self, guild_id: String) -> Option<VoiceState> {
        if self.voice_states.contains_key(&guild_id)
            && self.voice_states[&guild_id].contains_key(self.user.as_ref().unwrap().id.as_str())
        {
            return Some(
                self.voice_states[&guild_id][self.user.as_ref().unwrap().id.as_str()].clone(),
            );
        }
        let req = reqwest::Client::new()
            .get(format!(
                "https://discord.com/api/v10/guilds/{}/voice-states/@me",
                guild_id
            ))
            .header("Authorization", format!("Bot {}", self.token))
            .send()
            .await;

        if let Ok(response) = req {
            if response.status().is_success() {
                let body = response.text().await.unwrap();
                let json_message = serde_json::from_str::<serde_json::Value>(&body).unwrap();
                let voice_state = VoiceState::new(json_message);
                self.voice_states
                    .entry(voice_state.guild_id.clone())
                    .or_insert(HashMap::new())
                    .insert(voice_state.user_id.clone(), voice_state.clone());
                return Some(voice_state.clone());
            } else {
                println!(
                    "Error checking voice channels: {}",
                    response.text().await.unwrap()
                );
                return None;
            }
        } else {
            println!("Error checking voice channels");
            return None;
        }
    }

    pub async fn join_voice(
        &mut self,
        guild_id: String,
        channel_id: String,
        sender: Sender<axum::extract::ws::Message>,
    ) {
        let connection =
            Connection::new(sender.clone(), guild_id.clone(), channel_id.clone()).await;
        self.driver.insert(guild_id.clone(), connection);
        let join_voice = json!({
            "op":4,
            "d": {
                "guild_id": guild_id,
                "channel_id": channel_id,
                "self_mute": false,
                "self_deaf": false,
            }
        });
        self.sender
            .send(Message::Text(join_voice.to_string().into()))
            .await
            .unwrap();
    }

    pub async fn leave_voice(&mut self, guild_id: String) {
        let dr = self.driver.get_mut(&guild_id);
        if let Some(dr) = dr {
            dr.driver.lock().await.leave();
        }
        self.driver.remove(&guild_id);
        self.joined_guilds.remove(&guild_id);
    }

    pub async fn handle_ws(&mut self) {
        let identify_body = json!({
        "op": 2,
        "d": {
            "token": self.token,
            "intents": 33409,
            "properties": {"$os": "linux", "$browser": "songbird", "$device": "songbird"},
            "presence": {
              "status": "online",
              "activities": [
                  {
                      "id": "741124112411241124",
                      "name": "Songbird",
                      "type": 3,
                      "state": "Listening to a song",
                      "details": "Songbird",
                  }
              ]
            }
          }
        });
        self.sender
            .send(Message::Text(identify_body.to_string().into()))
            .await
            .unwrap();
        let receiver = self.receiver.clone();
        while let Ok(message) = receiver.recv().await {
            if message.is_text() {
                let json_message = serde_json::from_str::<serde_json::Value>(&message.to_string());
                if let Ok(json_message) = json_message {
                    if json_message["op"] == 10 {
                        let interval = json_message["d"]["heartbeat_interval"].as_u64().unwrap();
                        self.heartbeat_interval = interval;
                        let mut client = self.clone();
                        tokio::spawn(async move {
                            client.heartbeat().await;
                        });
                    } else if json_message["op"] == 11 {
                        // heartbeat response
                    } else if json_message["op"] == 0 {
                        match json_message["t"].as_str() {
                            Some("MESSAGE_CREATE") => {
                                let content = json_message["d"]["content"].as_str().unwrap();
                                // println!("message : {}", content);
                            }
                            Some("GUILD_CREATE") => {
                                if let Some(guild_id) = json_message["d"]["id"].as_str() {
                                    match Guild::new(json_message["d"].clone()) {
                                        Ok(guild) => {
                                            self.guilds.insert(guild_id.to_string(), guild);
                                            self.guilds_ids.insert(guild_id.to_string());
                                        }
                                        Err(e) => println!("Error creating guild: {}", e),
                                    }
                                }
                            }
                            Some("READY") => {
                                self.session_id = json_message["d"]["session_id"]
                                    .as_str()
                                    .unwrap_or_default()
                                    .to_string();

                                self.resume_gateway_url = json_message["d"]["resume_gateway_url"]
                                    .as_str()
                                    .unwrap_or_default()
                                    .to_string();

                                if let Some(user_data) = json_message["d"]["user"].as_object() {
                                    match User::new(Value::Object(user_data.clone())) {
                                        Ok(user) => self.user = Some(user),
                                        Err(e) => println!("Error creating user: {}", e),
                                    }
                                } else {
                                    println!("Warning: No user data found in READY event");
                                }
                                println!(
                                    "Reading userId: {} as username: {}",
                                    self.user.as_ref().unwrap().id,
                                    self.user.as_ref().unwrap().username
                                );
                                if let Some(guilds) = json_message["d"]["guilds"].as_array() {
                                    for guild in guilds {
                                        if let Some(guild_id) = guild["id"].as_str() {
                                            self.guilds_ids.insert(guild_id.to_string());
                                        }
                                    }
                                }

                                self.is_connected = true;
                                self.event_sender
                                    .send(axum::extract::ws::Message::Text(
                                        json!({
                                            "op": 4043,
                                            "t": "READY",
                                            "d":{
                                                "user":self.user.clone().unwrap().raw
                                            }
                                        })
                                        .to_string(),
                                    ))
                                    .await
                                    .unwrap();
                            }
                            Some("VOICE_STATE_UPDATE") => {
                                let voice_state = VoiceState::new(json_message["d"].clone());
                                self.voice_states
                                    .entry(voice_state.guild_id.clone())
                                    .or_insert(HashMap::new())
                                    .insert(voice_state.user_id.clone(), voice_state);
                            }
                            Some("VOICE_SERVER_UPDATE") => {
                                println!("VOICE_SERVER_UPDATE");
                                let endpoint =
                                    json_message["d"]["endpoint"].as_str().unwrap().to_string();
                                let token =
                                    json_message["d"]["token"].as_str().unwrap().to_string();
                                let guild_id =
                                    json_message["d"]["guild_id"].as_str().unwrap().to_string();
                                let connection = self.driver.get_mut(&guild_id).unwrap();
                                self.joined_guilds.insert(guild_id.clone());
                                connection
                                    .connect(ConnectionInfo {
                                        channel_id: Some(ChannelId(
                                            NonZeroU64::new(
                                                connection.channel_id.parse::<u64>().unwrap(),
                                            )
                                            .unwrap(),
                                        )),
                                        endpoint: endpoint.to_string(),
                                        guild_id: GuildId(
                                            NonZeroU64::new(guild_id.parse::<u64>().unwrap())
                                                .unwrap(),
                                        ),
                                        session_id: self.session_id.clone(),
                                        token: token.clone(),
                                        user_id: RawUserId(
                                            NonZeroU64::new(
                                                self.user
                                                    .clone()
                                                    .unwrap()
                                                    .id
                                                    .parse::<u64>()
                                                    .unwrap(),
                                            )
                                            .unwrap(),
                                        ),
                                    })
                                    .await
                                    .unwrap();
                            }
                            _ => {
                                // println!(
                                //     "not handled event : {}",
                                //     serde_json::to_string_pretty(&json_message).unwrap()
                                // );
                            }
                        }
                    } else {
                        println!(
                            "not handled op code : {}",
                            serde_json::to_string(&json_message).unwrap()
                        );
                    }
                }
            }
        }
    }
    pub async fn heartbeat(&mut self) {
        loop {
            let heartbeat_body = json!({
                "op": 1,
                "d": self.heartbeat_interval
            });
            self.sender
                .send(Message::Text(heartbeat_body.to_string().into()))
                .await
                .unwrap();
            tokio::time::sleep(Duration::from_millis(self.heartbeat_interval as u64)).await;
        }
    }

    pub async fn play(&mut self, guild_id: String, channel_id: String, url: String) {
        let connection = self.driver.get_mut(&guild_id).unwrap();
        if connection.channel_id == channel_id {
            connection.play(url).await;
        }
    }

    pub async fn pause(&mut self, channel_id: String, guild_id: String) {
        let connection = self.driver.get_mut(&guild_id).unwrap();
        if channel_id == connection.channel_id {
            connection.pause().await;
        }
    }

    pub async fn resume(&mut self, channel_id: String, guild_id: String) {
        let connection = self.driver.get_mut(&guild_id).unwrap();
        if channel_id == connection.channel_id {
            connection.resume().await;
        }
    }

    pub async fn stop(&mut self, channel_id: String, guild_id: String) {
        let connection = self.driver.get_mut(&guild_id).unwrap();
        if channel_id == connection.channel_id {
            connection.stop().await;
        }
    }

    pub async fn volume(&mut self, channel_id: String, guild_id: String, volume: f32) {
        let connection = self.driver.get_mut(&guild_id).unwrap();
        if channel_id == connection.channel_id {
            connection.volume(volume).await;
        }
    }

    pub async fn can_join_voice(&mut self, guild_id: String) -> bool {
        let voice_state = self.check_voice_channels(guild_id.clone()).await;
        // bot not currently in any voice channel of the guild
        return !self.driver.get_mut(&guild_id).is_some()
            // bot is connected to discord
            && self.is_connected
            // bot is in the guild
            && self.guilds.contains_key(&guild_id)
            // bot is not already in the voice channel
            && !self.joined_guilds.contains(&guild_id)
            // bot is not in the voice channel
            && !voice_state.is_some();
    }
}
