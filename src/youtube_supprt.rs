use crate::get_input;
use songbird::input::Input;
use rustube::{VideoFetcher, Id};

pub async fn youtube_modun(url: String) -> Result<Input, ()> {
    let id = Id::from_raw(&url);
    if id.is_err() {
        return Err(());
    }
    let descrambler = VideoFetcher::from_id(id.unwrap().into_owned()).unwrap().fetch().await;
    if descrambler.is_err() {
        return Err(());
    }
    let v = descrambler.unwrap().descramble().await;
    if v.is_err() {
        return Err(());
    }
    let v = v.unwrap();
    let mut best_audio_raw = v.best_audio();
    
    if best_audio_raw.is_none() {
        best_audio_raw = v.best_audio_mix();
        if best_audio_raw.is_none() { 
            return Err(());
        }
    }
    let out = best_audio_raw.unwrap();
    Ok(get_input(out.signature_cipher.url.to_string()).await)
}