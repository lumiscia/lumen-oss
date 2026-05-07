use std::{
    path::Path,
    sync::mpsc,
    thread::{self, JoinHandle},
};

use anyhow::anyhow;
use lumen::{
    audio::{AUDIO_CHANNELS, AUDIO_SAMPLE_RATE, AudioBuffer, AudioMixer, duration_samples},
    composition::Composition,
};
use lumen_ffmpeg::{
    AudioEncoderConfig, AudioFrame, CpuVideoFrame, MuxedEncoder, PixelFormat, SampleFormat,
    VideoCodec, VideoEncoderConfig,
};

use super::media::LocalMediaStore;

pub(super) const ENCODER_FRAME_QUEUE_CAPACITY: usize = 2;

pub(super) struct EncoderFrame {
    pub frame: u32,
    pub pixels: Vec<u8>,
    pub recycle_tx: mpsc::SyncSender<Vec<u8>>,
}

enum EncoderMessage {
    Video(EncoderFrame),
    Audio(AudioFrame),
}

pub(super) struct LumenFfmpegEncoder {
    message_tx: Option<mpsc::SyncSender<EncoderMessage>>,
    writer_handle: Option<JoinHandle<anyhow::Result<()>>>,
}

impl LumenFfmpegEncoder {
    pub(super) fn create(
        output: &Path,
        width: u32,
        height: u32,
        fps: f32,
        encoder: &str,
        codec: VideoCodec,
        include_audio: bool,
    ) -> anyhow::Result<Self> {
        let mut config =
            VideoEncoderConfig::cpu_rgba(width, height, fps.round().max(1.0) as u32, codec);
        config.encoder_name = Some(encoder.to_string());
        config.bit_rate = 14_000_000;
        let audio = include_audio
            .then(|| AudioEncoderConfig::aac(AUDIO_SAMPLE_RATE, AUDIO_CHANNELS as u16));

        let (message_tx, message_rx) =
            mpsc::sync_channel::<EncoderMessage>(ENCODER_FRAME_QUEUE_CAPACITY);
        let (startup_tx, startup_rx) = mpsc::sync_channel::<anyhow::Result<()>>(1);
        let output = output.to_string_lossy().to_string();
        let writer_handle = thread::spawn(move || {
            let mut encoder = match MuxedEncoder::create_with_audio(output, config, audio) {
                Ok(encoder) => {
                    let _ = startup_tx.send(Ok(()));
                    encoder
                }
                Err(error) => {
                    let _ = startup_tx.send(Err(anyhow!(
                        "lumen-ffmpeg encoder failed to start: {error}"
                    )));
                    return Ok(());
                }
            };
            while let Ok(message) = message_rx.recv() {
                match message {
                    EncoderMessage::Video(frame) => {
                        let frame_index = frame.frame;
                        let cpu_frame = CpuVideoFrame {
                            width,
                            height,
                            stride: (width as usize) * 4,
                            pixel_format: PixelFormat::Rgba8,
                            pts: Some(i64::from(frame_index)),
                            data: frame.pixels,
                        };
                        encoder.write_video_frame(&cpu_frame).map_err(|error| {
                            anyhow!("lumen-ffmpeg encode failed at frame {frame_index}: {error}")
                        })?;
                        let _ = frame.recycle_tx.try_send(cpu_frame.data);
                    }
                    EncoderMessage::Audio(frame) => {
                        encoder.write_audio_frame(&frame).map_err(|error| {
                            anyhow!("lumen-ffmpeg audio encode failed: {error}")
                        })?;
                    }
                }
            }
            encoder
                .finish()
                .map_err(|error| anyhow!("lumen-ffmpeg encoder finish failed: {error}"))
        });
        startup_rx
            .recv()
            .map_err(|_| anyhow!("lumen-ffmpeg encoder startup thread stopped"))??;
        Ok(Self {
            message_tx: Some(message_tx),
            writer_handle: Some(writer_handle),
        })
    }

    pub(super) fn send(&self, frame: EncoderFrame) -> anyhow::Result<()> {
        self.message_tx
            .as_ref()
            .ok_or_else(|| anyhow!("lumen-ffmpeg encoder writer unavailable"))?
            .send(EncoderMessage::Video(frame))
            .map_err(|_| anyhow!("lumen-ffmpeg encoder writer stopped"))
    }

    fn send_audio(&self, frame: AudioFrame) -> anyhow::Result<()> {
        self.message_tx
            .as_ref()
            .ok_or_else(|| anyhow!("lumen-ffmpeg encoder writer unavailable"))?
            .send(EncoderMessage::Audio(frame))
            .map_err(|_| anyhow!("lumen-ffmpeg encoder writer stopped"))
    }

    pub(super) fn finish(mut self) -> anyhow::Result<()> {
        self.message_tx.take();
        self.writer_handle
            .take()
            .ok_or_else(|| anyhow!("lumen-ffmpeg encoder writer thread missing"))?
            .join()
            .map_err(|_| anyhow!("lumen-ffmpeg encoder writer thread panicked"))?
    }
}

pub(super) fn write_composited_audio(
    composition: &Composition,
    media_store: &LocalMediaStore,
    encoder: &LumenFfmpegEncoder,
) -> anyhow::Result<()> {
    write_composited_audio_with(composition, media_store, |frame| encoder.send_audio(frame))
}

pub(super) fn write_composited_audio_with(
    composition: &Composition,
    media_store: &LocalMediaStore,
    mut write_frame: impl FnMut(AudioFrame) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    let Some(audio) = composition.audio.as_ref() else {
        return Ok(());
    };

    let total_samples = duration_samples(
        composition.timeline.duration_frames,
        composition.timeline.fps,
    );
    let mixer = AudioMixer::new(audio, media_store);
    let mut start_sample = 0_u64;
    let chunk_frames = (AUDIO_SAMPLE_RATE as usize).saturating_mul(30);

    while start_sample < total_samples {
        let frames = usize::try_from((total_samples - start_sample).min(chunk_frames as u64))
            .unwrap_or(chunk_frames);
        let mixed = mixer
            .mix_range(start_sample, frames)
            .map_err(|err| anyhow!("audio mix failed at sample {start_sample}: {err}"))?;
        write_frame(audio_frame_from_buffer(&mixed, start_sample))?;
        start_sample = start_sample.saturating_add(frames as u64);
    }

    Ok(())
}

pub(super) fn has_audio(composition: &Composition) -> bool {
    composition
        .audio
        .as_ref()
        .is_some_and(|audio| !audio.clips.is_empty())
}

fn audio_frame_from_buffer(buffer: &AudioBuffer, start_sample: u64) -> AudioFrame {
    AudioFrame {
        sample_rate: buffer.sample_rate(),
        channels: buffer.channel_count() as u16,
        sample_format: SampleFormat::F32,
        pts: Some(start_sample as i64),
        samples: buffer.frames(),
        interleaved_f32: buffer.interleaved_f32(),
    }
}
