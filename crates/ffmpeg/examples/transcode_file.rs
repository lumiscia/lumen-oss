use std::{env, path::PathBuf, time::Instant};

use lumen_ffmpeg::{
    DecodeMode, InputContext, MuxedEncoder, VideoDecoder, VideoDecoderConfig, VideoEncoderConfig,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    let input_path = args
        .next()
        .map(PathBuf::from)
        .ok_or("usage: transcode_file <input> <output> [max_frames] [encoder]")?;
    let output_path = args
        .next()
        .map(PathBuf::from)
        .ok_or("missing output path")?;
    let max_frames = args.next().and_then(|value| value.parse::<usize>().ok());
    let encoder_name = args.next();

    let started = Instant::now();
    let mut input = InputContext::open(input_path.to_string_lossy().to_string())?;
    let stream = input.best_video_stream()?;
    let mut decoder = VideoDecoder::open(
        &input,
        VideoDecoderConfig {
            stream_index: stream.stream_index,
            mode: DecodeMode::Cpu,
        },
    )?;
    let fps = stream
        .avg_frame_rate
        .as_f64()
        .filter(|fps| *fps > 0.0)
        .unwrap_or(60.0)
        .round()
        .clamp(1.0, u32::MAX as f64) as u32;
    let mut config = VideoEncoderConfig::h264_rgba(stream.width, stream.height, fps);
    if let Some(encoder_name) = encoder_name {
        config.encoder_name = Some(encoder_name);
    }
    let mut encoder = MuxedEncoder::create(output_path.to_string_lossy().to_string(), config)?;

    let mut frames = 0_usize;
    'decode: while let Some(packet) = input.read_packet()? {
        decoder.send_packet(&packet)?;
        while let Some(frame) = decoder.receive_cpu_frame()? {
            encoder.write_video_frame(&frame)?;
            frames = frames.saturating_add(1);
            if max_frames.is_some_and(|limit| frames >= limit) {
                break 'decode;
            }
        }
    }

    if max_frames.is_none_or(|limit| frames < limit) {
        decoder.send_eof()?;
        while let Some(frame) = decoder.receive_cpu_frame()? {
            encoder.write_video_frame(&frame)?;
            frames = frames.saturating_add(1);
            if max_frames.is_some_and(|limit| frames >= limit) {
                break;
            }
        }
    }

    encoder.finish()?;
    println!("input={}", input_path.display());
    println!("output={}", output_path.display());
    println!("frames={frames}");
    println!("elapsed_ms={}", started.elapsed().as_millis());
    println!(
        "fps={:.2}",
        frames as f64 / started.elapsed().as_secs_f64().max(1e-9)
    );
    Ok(())
}
