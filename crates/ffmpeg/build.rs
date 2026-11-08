use std::env;

fn main() {
    println!("cargo:rustc-check-cfg=cfg(ffmpeg_frame_cropping)");
    println!("cargo:rustc-check-cfg=cfg(ffmpeg_supported_config)");
    // `AV_PKT_DATA_FRAME_CROPPING` and `avcodec_get_supported_config` arrived in FFmpeg 7.1.
    if env::var("DEP_FFMPEG_FFMPEG_7_1").is_ok_and(|value| value == "true") {
        println!("cargo:rustc-cfg=ffmpeg_frame_cropping");
        println!("cargo:rustc-cfg=ffmpeg_supported_config");
    }
}
