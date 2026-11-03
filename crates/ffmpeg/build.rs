use std::env;

fn main() {
    println!("cargo:rustc-check-cfg=cfg(ffmpeg_frame_cropping)");
    // `AV_PKT_DATA_FRAME_CROPPING` arrived in FFmpeg 7.1.
    if env::var("DEP_FFMPEG_FFMPEG_7_1").is_ok_and(|value| value == "true") {
        println!("cargo:rustc-cfg=ffmpeg_frame_cropping");
    }
}
