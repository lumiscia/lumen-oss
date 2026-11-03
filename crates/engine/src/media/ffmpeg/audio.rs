use std::sync::{Arc, Mutex};

use lumen_ffmpeg::AudioRangeReader;

use crate::{
    audio::{AUDIO_CHANNELS, AudioBuffer, AudioResolver},
    error::MediaError,
};

pub struct FfmpegAudioResolver {
    source: String,
    reader: Mutex<AudioRangeReader>,
}

impl FfmpegAudioResolver {
    pub fn open(source: impl Into<String>) -> Result<Self, MediaError> {
        let source = source.into();
        let reader =
            AudioRangeReader::open(source.as_str()).map_err(|err| decode_error(&source, err))?;
        Ok(Self {
            source,
            reader: Mutex::new(reader),
        })
    }
}

impl AudioResolver for FfmpegAudioResolver {
    fn resolve_range(
        &self,
        start_sample: u64,
        frames: usize,
    ) -> Result<Arc<AudioBuffer>, MediaError> {
        let decoded = self
            .reader
            .lock()
            .map_err(|_| MediaError::Decode {
                media_source: self.source.clone(),
                details: "audio reader panicked".to_string(),
            })?
            .read(start_sample, frames)
            .map_err(|err| decode_error(&self.source, err))?;

        let channels = (0..AUDIO_CHANNELS)
            .map(|channel| {
                decoded
                    .interleaved_f32
                    .iter()
                    .skip(channel)
                    .step_by(AUDIO_CHANNELS)
                    .copied()
                    .collect()
            })
            .collect();
        Ok(Arc::new(AudioBuffer::from_channels(channels)))
    }
}

fn decode_error(source: &str, err: lumen_ffmpeg::FfmpegError) -> MediaError {
    MediaError::Decode {
        media_source: source.to_string(),
        details: format!("audio decode failed: {err}"),
    }
}

#[cfg(test)]
mod tests {
    use std::{
        process::Command,
        time::{SystemTime, UNIX_EPOCH},
    };

    use crate::audio::AudioResolver;

    use super::FfmpegAudioResolver;

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

        let decoded = FfmpegAudioResolver::open(wav)
            .expect("open wav")
            .resolve_range(0, 4800)
            .expect("decode wav");
        let playlist = FfmpegAudioResolver::open(playlist);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(decoded.frames(), 4800);
        assert!(playlist.is_err());
    }
}
