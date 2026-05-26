use crate::audio::AudioFrame;
use crate::gpu::GpuVideoInput;
use crate::video::CpuVideoFrame;
use crate::{FfmpegError, Result};

use super::audio::{AudioEncoder, AudioEncoderConfig};
use super::output::OutputContext;
use super::telemetry::GpuEncodeTelemetry;
use super::video::{VideoEncoder, VideoEncoderConfig};

pub struct MuxedEncoder {
    output: OutputContext,
    video: VideoEncoder,
    audio: Option<AudioEncoder>,
    wrote_header: bool,
}

impl MuxedEncoder {
    pub fn create(path: impl Into<String>, video: VideoEncoderConfig) -> Result<Self> {
        Self::create_with_audio(path, video, None)
    }

    pub fn create_with_audio(
        path: impl Into<String>,
        video: VideoEncoderConfig,
        audio: Option<AudioEncoderConfig>,
    ) -> Result<Self> {
        let mut output = OutputContext::create(path)?;
        let mut video = VideoEncoder::create(&mut output, video)?;
        let mut audio = audio
            .map(|config| AudioEncoder::create(&mut output, config))
            .transpose()?;
        output.write_header()?;
        video.refresh_stream_time_base(&output)?;
        if let Some(audio) = audio.as_mut() {
            audio.refresh_stream_time_base(&output)?;
        }
        Ok(Self {
            output,
            video,
            audio,
            wrote_header: true,
        })
    }

    pub fn write_video_frame(&mut self, frame: &CpuVideoFrame) -> Result<()> {
        self.video.send_cpu_frame(&mut self.output, frame)
    }

    pub fn write_gpu_frame(&mut self, frame: &GpuVideoInput<'_>) -> Result<()> {
        self.video.send_gpu_frame(&mut self.output, frame)
    }

    pub fn write_audio_frame(&mut self, frame: &AudioFrame) -> Result<()> {
        let Some(audio) = self.audio.as_mut() else {
            return Err(FfmpegError::new(
                "MuxedEncoder::write_audio_frame",
                "encoder was created without an audio stream",
            ));
        };
        audio.send_audio_frame(&mut self.output, frame)
    }

    pub fn gpu_telemetry(&self) -> &GpuEncodeTelemetry {
        self.video.gpu_telemetry()
    }

    pub fn finish(mut self) -> Result<()> {
        self.video.flush(&mut self.output)?;
        if let Some(audio) = self.audio.as_mut() {
            audio.flush(&mut self.output)?;
        }
        self.wrote_header = false;
        self.output.write_trailer()
    }
}
