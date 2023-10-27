use serde_json::json;
use tokio::{process::Command, io::AsyncReadExt};
use std::process::Stdio;
use tokio::sync::mpsc;
use futures_util::{StreamExt, SinkExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::{
    accept_hdr_async,
    tungstenite::{handshake::server::{Request, Response}, Message},
};

mod obj;

#[tokio::main]
async fn main() {
    let server = TcpListener::bind("127.0.0.1:8080").await.unwrap();

    while let Ok((stream, _)) = server.accept().await {
        tokio::spawn(accept_connection(stream));
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
    let mut ws_stream = accept_hdr_async(stream, callback)
        .await
        .expect("Error during the websocket handshake occurred");
    let mut session_id = "".to_string();
    let mut user_id = "".to_string();
    let mut newws;
    while let Some(msg) = ws_stream.next().await {
        let msg = msg.unwrap();
        if msg.is_text() {
            let out: serde_json::Value = serde_json::from_str(msg.to_text().unwrap()).unwrap();
            let mut data_out = "";
            if out["t"].is_string() {
                data_out = out["t"].as_str().unwrap();
            }
            if data_out == "VOICE_STATE_UPDATE" {
                let msg = out["d"].as_object().unwrap();
                let sid = msg.get("session_id").unwrap().as_str().unwrap();
                session_id = sid.to_string();
                let uid = msg.get("user_id").unwrap().as_str().unwrap();
                user_id = uid.to_string();
                let channel_id = msg.get("channel_id").unwrap().is_null();
                if channel_id {
                    ws_stream.close(None).await.unwrap();
                    break;
                }
                print!("{:?}\n\n", msg);
            } else if data_out == "VOICE_SERVER_UPDATE" {
                let msg = out["d"].as_object().unwrap();
                let token = msg.get("token").unwrap().as_str().unwrap();
                let guild_id = msg.get("guild_id").unwrap().as_str().unwrap();
                let endpoint = msg.get("endpoint").unwrap().as_str().unwrap();

                let send_channel = tokio::sync::mpsc::unbounded_channel();
                let read_channel = tokio::sync::mpsc::unbounded_channel();

                newws = obj::VoiceGateway::new(format!("wss://{}", endpoint), send_channel, read_channel).await;
                newws.authenticate(session_id.clone(), token.to_string(), user_id.clone(), guild_id.to_string()).await;
                print!("{:?}\n\n", msg);
                let data = newws.get_messages_op(2).await;
                println!("{}", data.unwrap().to_string());
            } else if data_out == "PING" {
                let send_smg = json!({"t": "PONG"});
                let raw_json = Message::Text(send_smg.to_string());
                ws_stream.send(raw_json).await.unwrap();
            }
        }
    }
}

#[allow(dead_code)]
async fn main_a() {
    let url = "https://m-api.fantasybot.tech/api/sp/spotify:track:2EjSewEKEZShOUBwFXUrK5?auth=50cad348f64323c6c9e2ff494a76c039";
    let key_rar = [60,103,26,90,167,61,107,5,234,239,149,125,183,219,20,167,21,110,216,86,121,109,44,244,214,21,80,243,152,206,77,157];
    let mut args_input:Vec<&str> = "-i".split(" ").collect();
    args_input.push(url);
    let mut args:Vec<&str> = "-f s16le -ar 48000 -ac 2 -loglevel warning -blocksize 1920".split(" ").collect();
    args.push("pipe:1");
    args_input.extend(args);
    let mut cmd = Command::new("ffmpeg")
        .args(args_input)
        .stdout(Stdio::piped())
        .spawn().unwrap();
    let mut stdout = cmd.stdout.take().unwrap();
    let mut objout = obj::VoiceEncode::new(key_rar, 352226);
    let mut objotp = obj::AudioOpus::new();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let task_rs = tokio::spawn(async move {
        let mut coutu = 0;
        loop {
            let out = rx.recv().await;
            println!("{coutu}:{}", out.is_none());
            if out.is_none() {
                break;
            }
            coutu += 1;
        }
    });
    let cl = tx.clone();
    let sendtask = tokio::spawn(async move {
        loop {
            let mut buffer = [0; 1920];
            let n = stdout.read(&mut buffer[..]).await.unwrap();
            if (n as u32) == 0 { break; }
            let sokeout = &buffer[..n];
            let mut f32_array: [f32; 1920] = [0.0; 1920];
            for i in 0..n {
                f32_array[i] = sokeout[i] as f32;
            }
            let out_opus = objotp.encode_audio(f32_array);
            let out = objout.encode_audio(out_opus.as_ref());
            cl.send(out).unwrap();  
        }
    });
    let outp = cmd.wait().await.unwrap();
    println!("{}", outp.code().unwrap());
    sendtask.abort();
    drop(tx);
    task_rs.await.unwrap();
}