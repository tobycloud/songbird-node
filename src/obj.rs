#![allow(dead_code)]
use std::time::Duration;

use crypto_secretbox::{
    aead::{Aead, KeyInit, Nonce},
    XSalsa20Poly1305, Key, SecretBox
};
use byteorder::{ByteOrder, NetworkEndian};
use audiopus::{coder::{Encoder, Decoder}, Channels, SampleRate, Application, Bitrate};
use discortp::{rtp::MutableRtpPacket, MutablePacket};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::sync::{mpsc::{UnboundedReceiver, UnboundedSender}, Mutex};
use tokio_tungstenite::{connect_async, tungstenite::Message};


pub const SAMPLE_RATE_RAW: u32 = 48000;
pub const AUDIO_FRAME_RATE: u32 = 50;
pub const BITRATE: i32 = 128000;
pub const MONO_FRAME_SIZE: u32 = SAMPLE_RATE_RAW / AUDIO_FRAME_RATE;
pub const STEREO_FRAME_SIZE: usize = (2 * MONO_FRAME_SIZE) as usize;
pub const TAG_SIZE: usize = SecretBox::<()>::TAG_SIZE;

#[allow(dead_code)]
struct _OpusStruct {
    pub sampling_rate: i32,
    pub channels: i32,
    pub frame_length: i32,
    pub sample_size: i32,
    pub samples_per_frame: i32,
    pub frame_size: i32,
}

impl _OpusStruct {
    fn new() -> _OpusStruct {
        let sampling_rate = 48000 as i32;
        let channels = 2 as i32;
        let frame_length = 20; // in ms
        let size = std::mem::size_of::<i16>() as i32;
        let sample_size = size * channels;
        let samples_per_frame = sampling_rate / 1000 * frame_length;
        let frame_size = samples_per_frame * sample_size;
        _OpusStruct{
            sampling_rate,
            channels,
            frame_length,
            sample_size,
            samples_per_frame,
            frame_size,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct VoiceEncode {
    sequence: u16, 
    timestamp: u32, 
    ssrc: u32,
    secret_key: [u8; 32],
}

const RTP_HEADER_SIZE: usize = 12;

impl VoiceEncode {

    fn serialize_rtp_header(self) -> [u8; RTP_HEADER_SIZE] {
        let mut header = [0u8; RTP_HEADER_SIZE];
        header[0] |= 2 << 6;
        header[1] |= 0 << 7;
        header[1] |= 0 << 6;
        header[1] |= 0 << 5;
        header[1] |= 0;
        NetworkEndian::write_u16(&mut header[2..4], self.sequence);
        NetworkEndian::write_u32(&mut header[4..8], self.timestamp);
        NetworkEndian::write_u32(&mut header[8..12], self.ssrc);
    
        header
    }

    pub fn new(secret_key: [u8; 32], ssrc: u32) -> VoiceEncode {
        VoiceEncode { sequence: 0, timestamp: 0, secret_key, ssrc}
    }

    pub fn encode_audio(&mut self, data: &[u8]) -> Vec<u8> {
        let headder = self.serialize_rtp_header();
        let key = Key::from_slice(&self.secret_key);
        let cipher = XSalsa20Poly1305::new(&key);
        let mut new_slice = Vec::new();
        new_slice.resize(24, 0);
        for i in 0..12 {
            new_slice[i] = headder[i];
        }
        let nonce = Nonce::<XSalsa20Poly1305>::from_slice(&new_slice.as_slice());
        let encode_out = cipher.encrypt(&nonce, data).unwrap();
        let mut vec_hd = headder.to_vec();
        vec_hd.extend(encode_out);
        vec_hd
    }


    pub fn audio_sended(&mut self) {
        let opt = _OpusStruct::new().samples_per_frame;
        let var = 1;
        if (self.sequence + var) >= 65535 {
            self.sequence = 0;
        } else {
            self.sequence += var;
        }
        let var: u32 = opt.try_into().unwrap();
        if (self.timestamp + var) >= 4294967295 {
            self.timestamp = 0;
        } else {
            self.timestamp += var;
        }
    }

}

pub struct AudioOpus {
    encode: Encoder,
    decode: Decoder
}

impl AudioOpus {
    pub fn new() -> AudioOpus {
        let channels = Channels::Stereo;
        let mode = Application::Audio;
        let sample_rate = SampleRate::Hz48000;
        let mut encode = Encoder::new(sample_rate, channels, mode).unwrap();
        encode.set_bitrate(Bitrate::BitsPerSecond(BITRATE)).unwrap();
        let decode = Decoder::new(sample_rate, channels).unwrap();
        AudioOpus {
            encode,
            decode
        }
    }

    pub fn encode_audio(&mut self, buffer: [f32; 1920]) -> Vec<u8> {
        let mut packet: [u8; 1460] = [0; 1460];
        let mut rtp = MutableRtpPacket::new(&mut packet[..]).expect(
            "FATAL: Too few bytes in self.packet for RTP header.\
                (Blame: VOICE_PACKET_MAX?)",
        );
        let payload = rtp.payload_mut();
        let total_payload_space = payload.len();
        let index = self.encode.encode_float(
            &buffer[..STEREO_FRAME_SIZE],
            &mut payload[TAG_SIZE..total_payload_space],
        ).unwrap();
        packet[..index].to_vec()
    }
}

pub struct VoiceGateway {
    send_tx: Mutex<UnboundedSender<Message>>,
    read_rx: Mutex<UnboundedReceiver<Message>>
}

impl VoiceGateway {
    pub async fn new(url: String, send_data: (UnboundedSender<Message>, UnboundedReceiver<Message>), read_data: (UnboundedSender<Message>, UnboundedReceiver<Message>)) -> VoiceGateway {
        let (ws_stream, _) = connect_async(url).await.expect("Failed to connect");
        let (mut write, mut read) = ws_stream.split();
        let (send_tx, mut  send_rx) = send_data;
        let (read_tx, read_rx) = read_data;
        tokio::spawn(async move {
            loop {
                let message = send_rx.recv().await;
                if message.is_none() {
                    break;
                }
                let check = write.send(message.unwrap()).await;
                if check.is_err() {
                    break;
                }
            }
        });
        let write_r = send_tx.clone();
        tokio::spawn(async move {
            while let Some(msg) = read.next().await {
                let msg = msg.unwrap();
                let td = msg.to_text().unwrap();
                let sd_r= serde_json::from_str(td);
                if sd_r.is_err() {
                    break;
                }
                let out:Value =  sd_r.unwrap();
                if out["op"].as_i64().unwrap() == 8 {
                    let heartbeat_interval = out["d"]["heartbeat_interval"].as_u64().unwrap();
                    let write = write_r.clone();
                    tokio::spawn(async move {
                        loop {
                            tokio::time::sleep(Duration::from_millis(heartbeat_interval)).await;
                            let data = json!({"op": 3, "d": Value::Null});
                            let out = write.send(Message::Text(data.to_string()));
                            if out.is_err() { break; }
                            println!("send heartbeat");            
                        }
                    });
                    continue;
                } else if out["op"].as_i64().unwrap() == 6 {
                    continue;
                }
                let out = read_tx.send(msg);
                if out.is_err() {
                    drop(write_r.clone());
                    println!("{:?}", out.unwrap_err().to_string());
                }
            }
        });
        VoiceGateway {send_tx: Mutex::new(send_tx.clone()), read_rx: Mutex::new(read_rx)}
    }

    pub async fn get_messages_op(&mut self, op_code: i64) -> Option<Message> {
        loop {
            let msg = self.read_rx.lock().await.recv().await;
            if msg.is_none() { return None; }
            let raw = msg.unwrap();
            let td = raw.to_text().unwrap();
            let out: serde_json::Value = serde_json::from_str(td).unwrap();
            if out["op"].as_i64().unwrap() == op_code { 
                return Some(raw);
            }
        }
    }

    pub async fn authenticate(&self, session_id: String, token:String, user_id:String, server_id: String) {
        let data = json!({
            "op": 0,
            "d": {
              "server_id": server_id,
              "user_id": user_id,
              "session_id": session_id,
              "token": token
            }
        });
        self.send_tx.lock().await.send(Message::Text(data.to_string())).unwrap();
    }

    pub async fn send_message(&self, data: Value) {
        self.send_tx.lock().await.send(Message::Text(data.to_string())).unwrap();
    }

}