#[cfg(target_os = "windows")]
pub(crate) fn get_ffmpeg_command() -> Vec<String> {
    [
        "-re",
        "-f",
        "gdigrab",
        "-i",
        "desktop",
        "-vf",
        "scale=1280:720",
        "-c:v",
        "vp8",
        "-pix_fmt",
        "yuv420p",
        "-r",
        "24",
        "-b:v",
        "2M",
        "-f",
        "ivf",
        "-preset",
        "ultrafast",
        "-",
    ]
    .into_iter()
    .map(String::from)
    .collect()
}

#[cfg(target_os = "linux")]
pub(crate) fn get_ffmpeg_command() -> Vec<String> {
    let display = std::env::var("DISPLAY").unwrap_or(String::from(":0"));

    [
        "-re",
        "-f",
        "x11grab",
        "-i",
        &format!("{display}.0"),
        "-vf",
        "scale=1280:720",
        "-c:v",
        "vp8",
        "-pix_fmt",
        "yuv420p",
        "-r",
        "24",
        "-b:v",
        "2M",
        "-f",
        "ivf",
        "-",
    ]
    .into_iter()
    .map(String::from)
    .collect()
}
