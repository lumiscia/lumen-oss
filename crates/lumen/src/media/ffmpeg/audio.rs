use std::{
    process::Command,
    sync::{Arc, Mutex},
};

use ffmpeg::media::Type;
use ffmpeg_next as ffmpeg;

use crate::{
    audio::{AUDIO_CHANNELS, AUDIO_SAMPLE_RATE, AudioBuffer, AudioMetadata, AudioResolver},
    error::MediaError,
};

use super::{ensure_ffmpeg_init, rational_to_f64};

pub struct FfmpegAudioResolver {
    id: String,
    metadata: AudioMetadata,
    decoded: Mutex<Option<Arc<AudioBuffer>>>,
}

impl FfmpegAudioResolver {
    pub fn open(source: impl Into<String>) -> Result<Self, MediaError> {
        ensure_ffmpeg_init()?;
        let source = source.into();
        let metadata = audio_metadata(&source)?;

        Ok(Self {
            id: source,
            metadata,
            decoded: Mutex::new(None),
        })
    }

    fn decoded(&self) -> Result<Arc<AudioBuffer>, MediaError> {
        if let Ok(cache) = self.decoded.lock()
            && let Some(buffer) = cache.as_ref()
        {
            return Ok(Arc::clone(buffer));
        }

        let buffer = Arc::new(decode_audio_to_f32_stereo(&self.id)?);
        if let Ok(mut cache) = self.decoded.lock() {
            *cache = Some(Arc::clone(&buffer));
        }
        Ok(buffer)
    }
}

impl AudioResolver for FfmpegAudioResolver {
    fn id(&self) -> &str {
        &self.id
    }

    fn metadata(&self) -> AudioMetadata {
        self.metadata
    }

    fn resolve_range(
        &self,
        start_sample: u64,
        frames: usize,
    ) -> Result<Arc<AudioBuffer>, MediaError> {
        let decoded = self.decoded()?;
        let start = usize::try_from(start_sample).unwrap_or(usize::MAX);
        let mut channels = vec![vec![0.0; frames]; AUDIO_CHANNELS];
        if start < decoded.frames() {
            let end = start.saturating_add(frames).min(decoded.frames());
            let copy_len = end.saturating_sub(start);
            for (channel_index, channel) in channels.iter_mut().enumerate() {
                let source = &decoded.channels()
                    [channel_index.min(decoded.channel_count().saturating_sub(1))];
                channel[..copy_len].copy_from_slice(&source[start..end]);
            }
        }

        Ok(Arc::new(AudioBuffer::from_channels(
            AUDIO_SAMPLE_RATE,
            channels,
        )))
    }
}

fn audio_metadata(source: &str) -> Result<AudioMetadata, MediaError> {
    let format = ffmpeg::format::input(source).map_err(|err| MediaError::Decode {
        media_source: source.to_string(),
        details: format!("failed opening audio source: {err}"),
    })?;
    let stream = format
        .streams()
        .best(Type::Audio)
        .ok_or_else(|| MediaError::SourceNotFound {
            media_source: source.to_string(),
        })?;
    let duration_seconds = if stream.duration() > 0 {
        rational_to_f64(stream.time_base())
            .map(|time_base| stream.duration() as f64 * time_base)
            .unwrap_or(0.0)
    } else if format.duration() > 0 {
        format.duration() as f64 / f64::from(ffmpeg::ffi::AV_TIME_BASE)
    } else {
        0.0
    };

    Ok(AudioMetadata {
        sample_rate: AUDIO_SAMPLE_RATE,
        channels: AUDIO_CHANNELS as u16,
        duration_samples: (duration_seconds.max(0.0) * f64::from(AUDIO_SAMPLE_RATE)).round() as u64,
    })
}

fn decode_audio_to_f32_stereo(source: &str) -> Result<AudioBuffer, MediaError> {
    let output = Command::new("ffmpeg")
        .arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-i")
        .arg(source)
        .arg("-vn")
        .arg("-f")
        .arg("f32le")
        .arg("-acodec")
        .arg("pcm_f32le")
        .arg("-ac")
        .arg(AUDIO_CHANNELS.to_string())
        .arg("-ar")
        .arg(AUDIO_SAMPLE_RATE.to_string())
        .arg("pipe:1")
        .output()
        .map_err(|err| MediaError::Decode {
            media_source: source.to_string(),
            details: format!("failed spawning ffmpeg audio decoder: {err}"),
        })?;

    if !output.status.success() {
        return Err(MediaError::Decode {
            media_source: source.to_string(),
            details: format!(
                "ffmpeg audio decode failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ),
        });
    }

    let sample_count = output.stdout.len() / std::mem::size_of::<f32>();
    let frame_count = sample_count / AUDIO_CHANNELS;
    let mut channels = vec![vec![0.0; frame_count]; AUDIO_CHANNELS];
    for frame in 0..frame_count {
        for (channel_index, channel) in channels.iter_mut().enumerate() {
            let byte_index = (frame * AUDIO_CHANNELS + channel_index) * 4;
            let bytes = [
                output.stdout[byte_index],
                output.stdout[byte_index + 1],
                output.stdout[byte_index + 2],
                output.stdout[byte_index + 3],
            ];
            channel[frame] = f32::from_le_bytes(bytes);
        }
    }

    Ok(AudioBuffer::from_channels(AUDIO_SAMPLE_RATE, channels))
}
