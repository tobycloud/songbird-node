use crate::discord::{DiscordClient, User};
use async_channel::{Receiver, Sender};
use std::collections::HashMap;
use tokio::task::JoinHandle;

#[derive(Clone, Debug)]
pub enum Command {
    JoinVoice(String, String, String),
    LeaveVoice(String),
    Play(String, String, String),
    Pause(String, String),
    Resume(String, String),
    Stop(String, String),
    Volume(String, String, f32),
    CanJoinVoice(String),
    JoinableVoice(String, bool),
}

#[derive(Clone)]
pub struct Client {
    pub user: User,
    pub rx: Receiver<axum::extract::ws::Message>,
    pub tx: Sender<axum::extract::ws::Message>,
    pub command_tx: Sender<Command>,
    pub command_rx: Receiver<Command>,
}

// #[derive(Clone)]
pub struct AppState {
    pub tokens: Vec<String>,
    pub tx: Sender<Command>,
    pub rx: Receiver<Command>,
    pub discord_clients: HashMap<String, Client>,
}

impl AppState {
    pub fn new() -> Self {
        let (tx, rx) = async_channel::unbounded::<Command>();

        Self {
            tokens: Vec::new(),
            tx,
            rx,
            discord_clients: HashMap::new(),
        }
    }

    pub async fn add_token(&mut self, token: String) {
        let (tx_data, rx_data) = async_channel::unbounded();
        let (command_tx, command_rx) = async_channel::unbounded();
        let tx_data_clone = tx_data.clone();
        {
            let tx = self.tx.clone();
            let rx = command_rx.clone();
            tokio::spawn(async move {
                let mut discord_client = DiscordClient::new(token, tx_data_clone.clone());
                discord_client.connect().await;
                while let Ok(message) = rx.recv().await {
                    match message {
                        Command::JoinVoice(guild_id, channel_id, user_id) => {
                            if let Some(user) = discord_client.user.as_ref() {
                                if user.id == user_id {
                                    discord_client
                                        .join_voice(guild_id, channel_id, tx_data_clone.clone())
                                        .await;
                                }
                            }
                        }
                        Command::LeaveVoice(guild_id) => {
                            discord_client.leave_voice(guild_id).await;
                        }
                        Command::Play(guild_id, channel_id, url) => {
                            discord_client.play(guild_id, channel_id, url).await;
                        }
                        Command::Pause(guild_id, channel_id) => {
                            discord_client.pause(guild_id, channel_id).await;
                        }
                        Command::Resume(guild_id, channel_id) => {
                            discord_client.resume(guild_id, channel_id).await;
                        }
                        Command::Stop(guild_id, channel_id) => {
                            discord_client.stop(guild_id, channel_id).await;
                        }
                        Command::Volume(guild_id, channel_id, volume) => {
                            discord_client.volume(guild_id, channel_id, volume).await;
                        }
                        Command::CanJoinVoice(guild_id) => {
                            println!(
                                "User : {:#?}, is ready: {}",
                                discord_client.user, discord_client.is_connected
                            );
                            if let Some(user) = discord_client.user.as_ref() {
                                println!(
                                    "userID: {}, is ready: {}",
                                    user.id, discord_client.is_connected
                                );
                                tx.send(Command::JoinableVoice(
                                    user.id.clone(),
                                    discord_client.can_join_voice(guild_id).await,
                                ))
                                .await
                                .unwrap();
                            } else {
                                println!("No user found");
                                tx.send(Command::JoinableVoice("".to_string(), false))
                                    .await
                                    .unwrap();
                            }
                        }
                        _ => {}
                    }
                }
            });
        }

        while let Ok(command) = rx_data.recv().await {
            match command {
                axum::extract::ws::Message::Text(text) => {
                    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
                    let op = json["op"].as_u64().unwrap();
                    // let t = json["t"].as_str().unwrap();
                    match op {
                        4043 => {
                            let user = json["d"]["user"].clone();
                            let user = User::new(user).unwrap();
                            self.discord_clients.insert(
                                user.id.clone(),
                                Client {
                                    user,
                                    rx: rx_data.clone(),
                                    tx: tx_data.clone(),
                                    command_rx: command_rx.clone(),
                                    command_tx: command_tx.clone(),
                                },
                            );
                            println!("Client added: {:?}", self.discord_clients.len());
                            return;
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    }

    pub async fn join_voice(
        &mut self,
        guild_id: String,
        channel_id: String,
    ) -> Result<Receiver<axum::extract::ws::Message>, String> {
        self.broadcast(Command::CanJoinVoice(guild_id.clone()))
            .await;
        let mut joinable_client_ids: Vec<String> = Vec::new();
        let mut i = 0;
        println!("Joining voice");
        while let Ok(command) = self.rx.recv().await {
            match command {
                Command::JoinableVoice(user_id, can_join) => {
                    println!("Joinable voice: {} {}", user_id, can_join);
                    i += 1;
                    if can_join {
                        joinable_client_ids.push(user_id);
                    }
                }
                _ => {
                    println!("Unknown command: {:#?}", command);
                }
            }
            println!("i: {}, len: {:?}", i, self.discord_clients.keys());
            if i >= self.discord_clients.len() {
                break;
            }
        }
        if joinable_client_ids.len() > 0 {
            // self.tx
            //     .send(Command::JoinVoice(
            //         guild_id.clone(),
            //         channel_id.clone(),
            //         joinable_client_ids[0].clone(),
            //     ))
            //     .await
            //     .unwrap();
            self.broadcast(Command::JoinVoice(
                guild_id.clone(),
                channel_id.clone(),
                joinable_client_ids[0].clone(),
            ))
            .await;
            return Ok(self
                .discord_clients
                .get(&joinable_client_ids[0])
                .unwrap()
                .rx
                .clone());
        }
        Err("No joinable client found".to_string())
    }

    pub async fn leave_voice(&self, guild_id: String) {
        // self.tx.send(Command::LeaveVoice(guild_id)).await.unwrap();
        self.broadcast(Command::LeaveVoice(guild_id)).await;
    }

    pub async fn play(&self, guild_id: String, channel_id: String, url: String) {
        // self.tx
        //     .send(Command::Play(guild_id, channel_id, url))
        //     .await
        //     .unwrap();
        self.broadcast(Command::Play(guild_id, channel_id, url))
            .await;
    }

    pub async fn pause(&self, channel_id: String, guild_id: String) {
        // self.tx
        //     .send(Command::Pause(guild_id, channel_id))
        //     .await
        //     .unwrap();
        self.broadcast(Command::Pause(guild_id, channel_id)).await;
    }

    pub async fn resume(&self, channel_id: String, guild_id: String) {
        // self.tx
        //     .send(Command::Resume(guild_id, channel_id))
        //     .await
        //     .unwrap();
        self.broadcast(Command::Resume(guild_id, channel_id)).await;
    }

    pub async fn stop(&self, channel_id: String, guild_id: String) {
        // self.tx
        //     .send(Command::Stop(guild_id, channel_id))
        //     .await
        //     .unwrap();
        self.broadcast(Command::Stop(guild_id, channel_id)).await;
    }

    pub async fn broadcast(&self, command: Command) {
        for (_, client) in self.discord_clients.iter() {
            client.command_tx.send(command.clone()).await.unwrap();
        }
    }
}
