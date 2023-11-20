use crate::ffmpeg_preconfig;
use songbird::input::{
    error::Result,
    Input,
    error::Error
};
use rustube::{VideoFetcher, Id};
use serde_json::Value;

pub async fn youtube_modun(url: String) -> Result<Input> {
    let id = Id::from_raw(&url);
    if id.is_err() {
        return Err(Error::YouTubeDlProcessing(Value::from("Url not valid")));
    }
    let descrambler = VideoFetcher::from_id(id.unwrap().into_owned()).unwrap().fetch().await;
    if descrambler.is_err() {
        return Err(Error::YouTubeDlProcessing(Value::from("Can get Video")));
    }
    let v = descrambler.unwrap().descramble().await;
    if v.is_err() {
        return Err(Error::YouTubeDlProcessing(Value::from("Can not Descramble Video")));
    }
    let v = v.unwrap();
    let mut best_audio_raw = v.best_audio();
    
    if best_audio_raw.is_none() {
        best_audio_raw = v.best_audio_mix();
        if best_audio_raw.is_none() { 
            return Err(Error::YouTubeDlProcessing(Value::from("Can not find audio in video")));
        }
    }
    let out = best_audio_raw.unwrap();
    println!("{}", out.signature_cipher.url);
    ffmpeg_preconfig(out.signature_cipher.url.as_str()).await
}