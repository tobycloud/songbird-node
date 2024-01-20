use std::{sync::atomic::AtomicBool, io::Cursor};

use async_trait::async_trait;
use axum::extract::ws::Message;
use base64::Engine as _;
use chrono::{DateTime, Utc};
use dashmap::DashMap;
use serde_json::json;
use songbird::{EventHandler, EventContext, Event, model::{id::UserId, payload::Speaking}};
use tokio::sync::mpsc::UnboundedSender;

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
pub struct Callback {
    pub ws: UnboundedSender<Message>
}

#[async_trait]
impl EventHandler for Callback {
    async fn act(&self, ctx: &EventContext<'_>) -> Option<Event> {
        match ctx {
            EventContext::Track(ts_raw) => {
                let ts = ts_raw.get(0).unwrap();
                let sucsess_stop = json!({
                    "t": "STOP"
                });
                let mess = match &ts.0.playing {
                    songbird::tracks::PlayMode::Play => {
                        json!({
                            "t": "P_STATE",
                            "d": true
                        })
                    },
                    songbird::tracks::PlayMode::Pause => {
                        json!({
                            "t": "P_STATE",
                            "d": false
                        })
                    }
                    songbird::tracks::PlayMode::Stop | songbird::tracks::PlayMode::End => {
                        sucsess_stop
                    }
                    songbird::tracks::PlayMode::Errored(err ) => {
                        println!("Error: {:?}", err);
                        json!({
                            "t": "STOP_ERROR"
                        })
                    },
                    _ => todo!(),   
                };
                self.ws.send(Message::Text(mess.to_string())).unwrap();
                
            },
            _ => return None,
        }
        None
    }
}



#[derive(Clone, Debug)]
pub struct Snippet {
    date: DateTime<Utc>,
    mapping: Option<UserInfo>,
}

#[derive(PartialEq, Copy, Clone, Debug)]
pub struct UserInfo {
    user_id: u64
}

pub struct InnerReceiver {
    last_tick_was_empty: AtomicBool,
    known_ssrcs: DashMap<u32, UserId>,
}

#[derive(Clone)]
pub struct CallbackR {
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