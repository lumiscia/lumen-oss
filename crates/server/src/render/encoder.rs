use std::{
    ops::Range,
    path::Path,
    sync::mpsc,
    thread::{self, JoinHandle},
};

use anyhow::{Context, anyhow};
use lumen_engine::{
    audio::{AUDIO_CHANNELS, AUDIO_SAMPLE_RATE, AudioBuffer, AudioMixer},
    composition::Composition,
};
use lumen_ffmpeg::{
    AudioEncoderConfig, AudioFrame, BitDepth, CpuVideoFrame, EncoderQuality, FfmpegError,
    MuxedEncoder, PixelFormat, Rational, VideoEncoderConfig,
};

use super::{CancelToken, media::LocalMediaStore};

const ENCODER_FRAME_QUEUE_CAPACITY: usize = 2;

enum EncoderMessage {
    Video(CpuVideoFrame),
    Audio(AudioFrame),
    Finish,
}

/// Encodes CPU frames on a writer thread. Sending after the writer has failed returns the writer's
/// own error, and dropping the sink without `finish` abandons the output.
pub(super) struct EncoderSink {
    message_tx: Option<mpsc::SyncSender<EncoderMessage>>,
    writer_handle: Option<JoinHandle<anyhow::Result<()>>>,
    input_format: PixelFormat,
}

impl EncoderSink {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn create(
        output: &Path,
        width: u32,
        height: u32,
        fps: f32,
        encoder: &str,
        quality: &EncoderQuality,
        bit_depth: BitDepth,
        alpha: bool,
        include_audio: bool,
    ) -> anyhow::Result<Self> {
        let mut config = VideoEncoderConfig::cpu_rgba(
            width,
            height,
            Rational::from_fps(f64::from(fps))?,
            encoder,
        );
        config.quality = quality.clone();
        config.bit_depth = bit_depth;
        config.alpha = alpha;
        let webm = output
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("webm"));
        let audio = include_audio.then(|| audio_config(webm));

        let (message_tx, message_rx) =
            mpsc::sync_channel::<EncoderMessage>(ENCODER_FRAME_QUEUE_CAPACITY);
        let (startup_tx, startup_rx) = mpsc::sync_channel::<Result<PixelFormat, FfmpegError>>(1);
        let output = output.to_string_lossy().to_string();
        let writer_handle = thread::spawn(move || {
            let mut encoder = match MuxedEncoder::create_with_audio(output, config, audio) {
                Ok(encoder) => {
                    warn_if_container_cropped(&encoder);
                    let format = encoder
                        .cpu_input_format()
                        .expect("a CPU upload encoder takes CPU frames");
                    let _ = startup_tx.send(Ok(format));
                    encoder
                }
                Err(error) => {
                    let _ = startup_tx.send(Err(error));
                    return Ok(());
                }
            };
            // An encoder dropped without `finish` belongs to a render that failed, whose output
            // nothing will read.
            while let Ok(message) = message_rx.recv() {
                match message {
                    EncoderMessage::Video(cpu_frame) => {
                        encoder.write_video_frame(&cpu_frame).map_err(|error| {
                            anyhow!(
                                "lumen-ffmpeg encode failed at frame {}: {error}",
                                cpu_frame.pts.unwrap_or_default()
                            )
                        })?;
                    }
                    EncoderMessage::Audio(frame) => {
                        encoder.write_audio_frame(&frame).map_err(|error| {
                            anyhow!("lumen-ffmpeg audio encode failed: {error}")
                        })?;
                    }
                    EncoderMessage::Finish => {
                        return encoder.finish().map_err(|error| {
                            anyhow!("lumen-ffmpeg encoder finish failed: {error}")
                        });
                    }
                }
            }
            Ok(())
        });
        let input_format = startup_rx
            .recv()
            .map_err(|_| anyhow!("lumen-ffmpeg encoder startup thread stopped"))?
            .map_err(anyhow::Error::new)?;
        Ok(Self {
            message_tx: Some(message_tx),
            writer_handle: Some(writer_handle),
            input_format,
        })
    }

    /// The YUV format the encoder takes frames in.
    pub(super) fn input_format(&self) -> PixelFormat {
        self.input_format
    }

    pub(super) fn send_frame(&mut self, frame: CpuVideoFrame) -> anyhow::Result<()> {
        self.send(EncoderMessage::Video(frame))
    }

    pub(super) fn send_audio(&mut self, frame: AudioFrame) -> anyhow::Result<()> {
        self.send(EncoderMessage::Audio(frame))
    }

    pub(super) fn finish(mut self) -> anyhow::Result<()> {
        // A writer that already failed has stopped reading, and the join below reports why.
        if let Some(message_tx) = self.message_tx.take() {
            let _ = message_tx.send(EncoderMessage::Finish);
        }
        self.join()
    }

    fn send(&mut self, message: EncoderMessage) -> anyhow::Result<()> {
        let sent = self
            .message_tx
            .as_ref()
            .is_some_and(|message_tx| message_tx.send(message).is_ok());
        if sent {
            return Ok(());
        }
        self.message_tx.take();
        self.join()?;
        Err(anyhow!("lumen-ffmpeg encoder writer stopped"))
    }

    fn join(&mut self) -> anyhow::Result<()> {
        match self.writer_handle.take() {
            Some(writer) => writer
                .join()
                .map_err(|_| anyhow!("lumen-ffmpeg encoder writer thread panicked"))?,
            None => Ok(()),
        }
    }
}

impl Drop for EncoderSink {
    fn drop(&mut self) {
        self.message_tx.take();
        let _ = self.join();
    }
}

/// The audio a video holds: Opus in a WebM, which takes nothing else, and AAC in the rest.
pub(super) fn audio_config(webm: bool) -> AudioEncoderConfig {
    let channels = AUDIO_CHANNELS as u16;
    if webm {
        AudioEncoderConfig::opus(AUDIO_SAMPLE_RATE, channels)
    } else {
        AudioEncoderConfig::aac(AUDIO_SAMPLE_RATE, channels)
    }
}

pub(super) fn warn_if_container_cropped(encoder: &MuxedEncoder) {
    if let Some(crop) = encoder.container_crop() {
        tracing::warn!(
            coded_width = crop.coded_width,
            coded_height = crop.coded_height,
            width = crop.width,
            height = crop.height,
            "encoder padded the canvas and the container crops it back; players that ignore the \
             crop (including Chromium) show the padding, so render at the coded size to avoid this"
        );
    }
}

/// Why the audio stopped before its last chunk: the render was cancelled.
#[derive(Debug)]
pub(super) struct AudioCancelled;

impl std::fmt::Display for AudioCancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the render was cancelled")
    }
}

impl std::error::Error for AudioCancelled {}

/// The sample `frame` starts at, at the rate the video is encoded with, so audio and video cover
/// the same time.
fn frame_sample(frame: u32, fps: Rational) -> u64 {
    let (numerator, denominator) = (fps.numerator as u128, fps.denominator as u128);
    let samples = u128::from(frame) * denominator * u128::from(AUDIO_SAMPLE_RATE);
    ((samples * 2 + numerator) / (numerator * 2)) as u64
}

/// Mixes the audio of `frames`, a range of the timeline's frames, a chunk at a time. The output
/// starts at the sample the first frame starts at and ends where the last one does, so it lasts as
/// long as the frames do. Stops with [`AudioCancelled`] before the chunk that follows a
/// cancellation.
pub(super) fn write_composited_audio(
    composition: &Composition,
    media_store: &LocalMediaStore,
    frames: &Range<u32>,
    cancel: &CancelToken,
    mut write_frame: impl FnMut(AudioFrame) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    if !has_audio(composition) {
        return Ok(());
    }

    let fps = Rational::from_fps(f64::from(composition.timeline().fps))?;
    let end_sample = frame_sample(frames.end, fps);
    let mut mixer = AudioMixer::new(composition, media_store);
    let mut start_sample = frame_sample(frames.start, fps);
    let chunk_frames = (AUDIO_SAMPLE_RATE as usize).saturating_mul(30);

    while start_sample < end_sample {
        if cancel.is_cancelled() {
            return Err(AudioCancelled.into());
        }
        let frames = usize::try_from((end_sample - start_sample).min(chunk_frames as u64))
            .unwrap_or(chunk_frames);
        let mixed = mixer
            .mix_range(start_sample, frames)
            .with_context(|| format!("audio mix failed at sample {start_sample}"))?;
        write_frame(audio_frame_from_buffer(&mixed))?;
        start_sample = start_sample.saturating_add(frames as u64);
    }

    Ok(())
}

pub(super) fn has_audio(composition: &Composition) -> bool {
    composition.has_audio()
}

fn audio_frame_from_buffer(buffer: &AudioBuffer) -> AudioFrame {
    AudioFrame {
        sample_rate: AUDIO_SAMPLE_RATE,
        channels: buffer.channel_count() as u16,
        samples: buffer.frames(),
        interleaved_f32: buffer.interleaved_f32(),
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        time::Duration,
    };

    use super::*;

    #[test]
    fn dropping_a_sink_waits_for_its_writer_to_close() {
        let (message_tx, message_rx) = mpsc::sync_channel::<EncoderMessage>(1);
        let closed = Arc::new(AtomicBool::new(false));
        let writer_closed = Arc::clone(&closed);
        let writer_handle = thread::spawn(move || {
            while message_rx.recv().is_ok() {}
            thread::sleep(Duration::from_millis(50));
            writer_closed.store(true, Ordering::SeqCst);
            Ok(())
        });
        drop(EncoderSink {
            message_tx: Some(message_tx),
            writer_handle: Some(writer_handle),
            input_format: PixelFormat::Yuv420p,
        });
        assert!(closed.load(Ordering::SeqCst));
    }

    #[test]
    fn a_frame_starts_at_the_sample_the_encoded_video_reaches() {
        let ntsc = Rational::from_fps(29.97).unwrap();
        assert_eq!(
            frame_sample(60_000, ntsc) - frame_sample(30_000, ntsc),
            48_048_000
        );
        assert_eq!(frame_sample(1, Rational::new(30, 1)), 1_600);
    }

    #[test]
    fn a_failed_writer_reports_its_own_error_to_the_sender() {
        let directory = tempfile::tempdir().unwrap();
        let mut sink = EncoderSink::create(
            &directory.path().join("out.mp4"),
            64,
            64,
            30.0,
            "libx264",
            &EncoderQuality::default(),
            BitDepth::Eight,
            false,
            false,
        )
        .unwrap();
        assert_eq!(sink.input_format(), PixelFormat::Yuv420p);

        // The writer rejects a frame of the wrong size, then stops reading.
        let error = (0..16)
            .find_map(|frame| {
                sink.send_frame(CpuVideoFrame {
                    width: 64,
                    height: 64,
                    stride: 256,
                    pixel_format: PixelFormat::Yuv420p,
                    pts: Some(i64::from(frame)),
                    data: Vec::new(),
                })
                .err()
            })
            .expect("the writer should fail")
            .to_string();

        assert!(error.contains("encode failed at frame 0"), "{error}");
    }
}
