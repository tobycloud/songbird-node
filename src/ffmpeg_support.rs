use songbird::input::{
    children_to_reader,
    error::Result,
    Codec,
    Container,
    Input,
};
use std::{
    ffi::OsStr,
    process::{Command, Stdio},
};

pub async fn ffmpeg_preconfig<P: AsRef<OsStr>>(path: P) -> Result<Input> {
    let path = path.as_ref();
    let is_stereo = true;
    let stereo_val = if is_stereo { "2" } else { "1" };
    let options = &["-vn"];
    let pre_input_args = vec![
        "-reconnect",
        "1",
        "-reconnect_streamed",
        "1",
        "-reconnect_delay_max",
        "10"];
    let mut args = vec![
        "-f",
        "s16le",
        "-ac",
        stereo_val,
        "-ar",
        "48000",
        "-acodec",
        "pcm_f32le",
        "-"];
    args.extend(options);
    let command = Command::new("ffmpeg")
        .args(pre_input_args)
        .arg("-i")
        .arg(path)
        .args(args)
        .stderr(Stdio::null())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()?;

    Ok(Input::new(
        is_stereo,
        children_to_reader::<f32>(vec![command]),
        Codec::FloatPcm,
        Container::Raw,
        None,
    ))
}