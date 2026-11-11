use std::{
    path::Path,
    sync::mpsc,
    thread::{self, JoinHandle},
};

use anyhow::{Context, anyhow};
use lumen_engine::{
    audio::{AUDIO_CHANNELS, AUDIO_SAMPLE_RATE, AudioBuffer, AudioMixer, duration_samples},
    composition::Composition,
    error::LumenError,
};
use lumen_ffmpeg::{
    AudioEncoderConfig, AudioFrame, CpuVideoFrame, EncoderQuality, FfmpegError, MuxedEncoder,
    PixelFormat, Rational, VideoEncoderConfig,
};

use super::media::LocalMediaStore;

const ENCODER_FRAME_QUEUE_CAPACITY: usize = 2;

enum EncoderMessage {
    Video { frame: u32, pixels: Vec<u8> },
    Audio(AudioFrame),
    Finish,
}

/// Encodes CPU frames on a writer thread. Sending after the writer has failed returns the writer's
/// own error, and dropping the sink without `finish` abandons the output.
pub(super) struct EncoderSink {
    message_tx: Option<mpsc::SyncSender<EncoderMessage>>,
    writer_handle: Option<JoinHandle<anyhow::Result<()>>>,
}

impl EncoderSink {
    pub(super) fn create(
        output: &Path,
        width: u32,
        height: u32,
        fps: f32,
        encoder: &str,
        quality: &EncoderQuality,
        include_audio: bool,
    ) -> anyhow::Result<Self> {
        let mut config = VideoEncoderConfig::cpu_rgba(
            width,
            height,
            Rational::from_fps(f64::from(fps))?,
            encoder,
        );
        config.quality = quality.clone();
        let audio = include_audio
            .then(|| AudioEncoderConfig::aac(AUDIO_SAMPLE_RATE, AUDIO_CHANNELS as u16));

        let (message_tx, message_rx) =
            mpsc::sync_channel::<EncoderMessage>(ENCODER_FRAME_QUEUE_CAPACITY);
        let (startup_tx, startup_rx) = mpsc::sync_channel::<Result<(), FfmpegError>>(1);
        let output = output.to_string_lossy().to_string();
        let writer_handle = thread::spawn(move || {
            let mut encoder = match MuxedEncoder::create_with_audio(output, config, audio) {
                Ok(encoder) => {
                    warn_if_container_cropped(&encoder);
                    let _ = startup_tx.send(Ok(()));
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
                    EncoderMessage::Video { frame, pixels } => {
                        let cpu_frame = CpuVideoFrame {
                            width,
                            height,
                            stride: (width as usize) * 4,
                            pixel_format: PixelFormat::Rgba8,
                            pts: Some(i64::from(frame)),
                            data: pixels,
                        };
                        encoder.write_video_frame(&cpu_frame).map_err(|error| {
                            anyhow!("lumen-ffmpeg encode failed at frame {frame}: {error}")
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
        startup_rx
            .recv()
            .map_err(|_| anyhow!("lumen-ffmpeg encoder startup thread stopped"))?
            .map_err(anyhow::Error::new)?;
        Ok(Self {
            message_tx: Some(message_tx),
            writer_handle: Some(writer_handle),
        })
    }

    pub(super) fn send_frame(&mut self, frame: u32, pixels: Vec<u8>) -> anyhow::Result<()> {
        self.send(EncoderMessage::Video { frame, pixels })
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

pub(super) fn write_composited_audio(
    composition: &Composition,
    media_store: &LocalMediaStore,
    mut write_frame: impl FnMut(AudioFrame) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    let Some(audio) = composition.audio().filter(|_| has_audio(composition)) else {
        return Ok(());
    };

    let total_samples = duration_samples(
        composition.timeline().duration_frames,
        composition.timeline().fps,
    );
    let mixer = AudioMixer::new(audio, media_store);
    let mut start_sample = 0_u64;
    let chunk_frames = (AUDIO_SAMPLE_RATE as usize).saturating_mul(30);

    while start_sample < total_samples {
        let frames = usize::try_from((total_samples - start_sample).min(chunk_frames as u64))
            .unwrap_or(chunk_frames);
        let mixed = mixer
            .mix_range(start_sample, frames)
            .map_err(LumenError::from)
            .with_context(|| format!("audio mix failed at sample {start_sample}"))?;
        write_frame(audio_frame_from_buffer(&mixed))?;
        start_sample = start_sample.saturating_add(frames as u64);
    }

    Ok(())
}

pub(super) fn has_audio(composition: &Composition) -> bool {
    composition
        .audio()
        .as_ref()
        .is_some_and(|audio| !audio.clips.is_empty())
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
        });
        assert!(closed.load(Ordering::SeqCst));
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
            false,
        )
        .unwrap();

        // The writer rejects a frame of the wrong size, then stops reading.
        let error = (0..16)
            .find_map(|frame| sink.send_frame(frame, Vec::new()).err())
            .expect("the writer should fail")
            .to_string();

        assert!(error.contains("encode failed at frame 0"), "{error}");
    }
}
