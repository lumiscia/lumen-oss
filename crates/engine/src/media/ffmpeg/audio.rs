use std::{
    process::Command,
    sync::{Arc, Mutex},
};

use ffmpeg::media::Type;
use ffmpeg_next as ffmpeg;
use lumen_ffmpeg::{ALLOWED_INPUT_FORMATS, ALLOWED_INPUT_PROTOCOLS};

use crate::{
    audio::{AUDIO_CHANNELS, AUDIO_SAMPLE_RATE, AudioBuffer, AudioResolver},
    error::MediaError,
};

use super::ensure_ffmpeg_init;

pub struct FfmpegAudioResolver {
    source: String,
    decoded_ranges: Mutex<Vec<CachedAudioRange>>,
}

impl FfmpegAudioResolver {
    const DECODE_WINDOW_SAMPLES: usize = AUDIO_SAMPLE_RATE as usize * 30;

    pub fn open(source: impl Into<String>) -> Result<Self, MediaError> {
        ensure_ffmpeg_init()?;
        let source = source.into();
        ensure_audio_stream(&source)?;

        Ok(Self {
            source,
            decoded_ranges: Mutex::new(Vec::new()),
        })
    }

    fn cached_range(&self, start_sample: u64, frames: usize) -> Option<ResolvedAudioRange> {
        let cache = self.decoded_ranges.lock().ok()?;
        cache
            .iter()
            .find(|range| range.covers(start_sample, frames))
            .map(CachedAudioRange::resolved)
    }

    fn decode_range(
        &self,
        start_sample: u64,
        frames: usize,
    ) -> Result<ResolvedAudioRange, MediaError> {
        if let Some(range) = self.cached_range(start_sample, frames) {
            return Ok(range);
        }

        let frames_to_decode = frames.max(Self::DECODE_WINDOW_SAMPLES);
        let buffer = Arc::new(decode_audio_range_to_f32_stereo(
            &self.source,
            start_sample,
            frames_to_decode,
        )?);
        if let Ok(mut cache) = self.decoded_ranges.lock() {
            if let Some(existing) = cache
                .iter()
                .find(|range| range.covers(start_sample, frames))
            {
                return Ok(existing.resolved());
            }
            cache.push(CachedAudioRange {
                start_sample,
                buffer: Arc::clone(&buffer),
            });
        }

        Ok(ResolvedAudioRange {
            start_sample,
            buffer,
        })
    }
}

struct CachedAudioRange {
    start_sample: u64,
    buffer: Arc<AudioBuffer>,
}

impl CachedAudioRange {
    fn resolved(&self) -> ResolvedAudioRange {
        ResolvedAudioRange {
            start_sample: self.start_sample,
            buffer: Arc::clone(&self.buffer),
        }
    }

    fn covers(&self, start_sample: u64, frames: usize) -> bool {
        let end_sample = start_sample.saturating_add(frames as u64);
        let cached_end = self
            .start_sample
            .saturating_add(self.buffer.frames() as u64);
        start_sample >= self.start_sample && end_sample <= cached_end
    }
}

struct ResolvedAudioRange {
    start_sample: u64,
    buffer: Arc<AudioBuffer>,
}

impl AudioResolver for FfmpegAudioResolver {
    fn resolve_range(
        &self,
        start_sample: u64,
        frames: usize,
    ) -> Result<Arc<AudioBuffer>, MediaError> {
        let decoded = self.decode_range(start_sample, frames)?;
        let relative_start = start_sample.saturating_sub(decoded.start_sample);
        let start = usize::try_from(relative_start).unwrap_or(usize::MAX);
        let mut channels = vec![vec![0.0; frames]; AUDIO_CHANNELS];
        if start < decoded.buffer.frames() {
            let end = start.saturating_add(frames).min(decoded.buffer.frames());
            let copy_len = end.saturating_sub(start);
            for (channel_index, channel) in channels.iter_mut().enumerate() {
                let source = &decoded.buffer.channels()
                    [channel_index.min(decoded.buffer.channel_count().saturating_sub(1))];
                channel[..copy_len].copy_from_slice(&source[start..end]);
            }
        }

        Ok(Arc::new(AudioBuffer::from_channels(channels)))
    }
}

fn ensure_audio_stream(source: &str) -> Result<(), MediaError> {
    let mut options = ffmpeg::Dictionary::new();
    options.set("format_whitelist", ALLOWED_INPUT_FORMATS);
    options.set("protocol_whitelist", ALLOWED_INPUT_PROTOCOLS);
    let format = ffmpeg::format::input_with_dictionary(source, options).map_err(|err| {
        MediaError::Decode {
            media_source: source.to_string(),
            details: format!("failed opening audio source: {err}"),
        }
    })?;
    if format.streams().best(Type::Audio).is_none() {
        return Err(MediaError::Decode {
            media_source: source.to_string(),
            details: "file has no audio stream".to_string(),
        });
    }
    Ok(())
}

fn decode_audio_range_to_f32_stereo(
    source: &str,
    start_sample: u64,
    frames: usize,
) -> Result<AudioBuffer, MediaError> {
    let start_seconds = start_sample as f64 / f64::from(AUDIO_SAMPLE_RATE);
    let duration_seconds = frames as f64 / f64::from(AUDIO_SAMPLE_RATE);
    let output = Command::new("ffmpeg")
        .arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-ss")
        .arg(format!("{start_seconds:.9}"))
        .arg("-t")
        .arg(format!("{duration_seconds:.9}"))
        .arg("-format_whitelist")
        .arg(ALLOWED_INPUT_FORMATS)
        .arg("-protocol_whitelist")
        .arg(ALLOWED_INPUT_PROTOCOLS)
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

    Ok(AudioBuffer::from_channels(channels))
}

#[cfg(test)]
mod tests {
    use std::{
        process::Command,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::{decode_audio_range_to_f32_stereo, ensure_audio_stream};

    #[test]
    fn audio_paths_read_uploads_but_not_playlists() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("lumen_audio_allowlist_{unique}"));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let wav = dir.join("tone.wav");
        let segment = dir.join("tone.ts");
        for (path, codec) in [(&wav, "pcm_s16le"), (&segment, "aac")] {
            let generated = Command::new("ffmpeg")
                .args(["-y", "-hide_banner", "-loglevel", "error", "-f", "lavfi"])
                .args(["-i", "sine=frequency=440:duration=0.5", "-c:a", codec])
                .arg(path)
                .status();
            if !matches!(generated, Ok(status) if status.success()) {
                eprintln!("ffmpeg CLI unavailable; skipping audio input allowlist test");
                return;
            }
        }
        let playlist = dir.join("tone.m3u8");
        std::fs::write(
            &playlist,
            format!(
                "#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXTINF:0.5,\n{}\n#EXT-X-ENDLIST\n",
                segment.display()
            ),
        )
        .expect("write playlist");
        let wav = wav.to_str().expect("wav path");
        let playlist = playlist.to_str().expect("playlist path");

        assert!(ensure_audio_stream(wav).is_ok());
        let decoded = decode_audio_range_to_f32_stereo(wav, 0, 4800).expect("decode wav");
        assert!(decoded.frames() > 0);

        let stream = ensure_audio_stream(playlist);
        let decoded = decode_audio_range_to_f32_stereo(playlist, 0, 4800);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(stream.is_err());
        assert!(decoded.is_err());
    }
}
