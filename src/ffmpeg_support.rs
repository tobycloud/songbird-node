use songbird::input::Input;
use std::process::{Command, Stdio};

pub async fn get_input(path: String) -> Input {
    ffmpeg_player(path.clone()).await
}

pub async fn ffmpeg_player(path: String) -> Input {
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
        "f32le",
        "-ac",
        stereo_val,
        "-ar",
        "48000",
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
        .spawn().unwrap();
    let data = crate::task_support::ChildContainer::from(command);
    return Input::from(data);
}
