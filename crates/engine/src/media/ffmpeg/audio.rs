use std::sync::{Arc, Mutex};

use lumen_ffmpeg::AudioRangeReader;

use super::open_failure;
use crate::{
    audio::{AUDIO_CHANNELS, AudioBuffer, AudioResolver},
    error::{MediaError, MediaFailure},
    media::AssetKind,
};

/// Readers kept open per source. A seek can't land on exactly the samples a continuous read gives,
/// so clips that overlap on one source each keep a reader and never seek during playback.
const MAX_READERS: usize = 4;

pub struct FfmpegAudioResolver {
    source: String,
    /// Least recently used first.
    readers: Mutex<Vec<AudioRangeReader>>,
}

impl FfmpegAudioResolver {
    pub fn open(source: impl Into<String>) -> Result<Self, MediaError> {
        let source = source.into();
        let reader = AudioRangeReader::open(source.as_str())
            .map_err(|err| open_failure(&source, AssetKind::Audio, &err))?;
        Ok(Self {
            source,
            readers: Mutex::new(vec![reader]),
        })
    }
}

impl AudioResolver for FfmpegAudioResolver {
    fn resolve_range(
        &self,
        start_sample: u64,
        frames: usize,
    ) -> Result<Arc<AudioBuffer>, MediaError> {
        let mut readers = self
            .readers
            .lock()
            .map_err(|_| MediaError::decode(&self.source, "audio reader panicked"))?;
        // The reader that is already there, else one that hasn't read yet, else a new one while
        // there's room, else the least recently used one, which seeks.
        let index = match [start_sample, 0].into_iter().find_map(|position| {
            readers
                .iter()
                .position(|reader| reader.position() == position)
        }) {
            Some(index) => index,
            None if readers.len() < MAX_READERS => {
                let reader = AudioRangeReader::open(self.source.as_str())
                    .map_err(|err| decode_error(&self.source, err))?;
                readers.push(reader);
                readers.len() - 1
            }
            None => 0,
        };
        let mut reader = readers.remove(index);
        let decoded = reader.read(start_sample, frames);
        readers.push(reader);
        let decoded = decoded.map_err(|err| decode_error(&self.source, err))?;
        drop(readers);

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
    let failure = MediaFailure::decode(source, format!("audio decode failed: {err}"))
        .wanting(&[AssetKind::Audio]);
    if err.is_resource_exhaustion() {
        failure.transient().into()
    } else {
        failure.into()
    }
}

#[cfg(test)]
mod tests {
    use std::{
        process::Command,
        time::{SystemTime, UNIX_EPOCH},
    };

    use crate::audio::AudioResolver;

    use lumen_ffmpeg::AudioRangeReader;

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

    /// Two clips of one source read in turn. A seek lands within a millisecond of the continuous
    /// read in Matroska, so each position must keep its own reader to match one read on its own.
    #[test]
    fn positions_read_in_turn_each_match_a_reader_of_their_own() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("lumen_audio_pool_{unique}.mkv"));
        let generated = Command::new("ffmpeg")
            .args(["-y", "-hide_banner", "-loglevel", "error", "-f", "lavfi"])
            .args(["-i", "aevalsrc=0.4*sin(2*PI*(300+200*t)*t):d=6:s=48000"])
            .args(["-c:a", "pcm_s16le"])
            .arg(&path)
            .status();
        if !matches!(generated, Ok(status) if status.success()) {
            eprintln!("ffmpeg CLI unavailable; skipping audio reader pool test");
            return;
        }
        let source = path.to_str().expect("path");
        const CHUNK: usize = 4_800;
        let starts = [12_345_u64, 100_000];

        let pooled = FfmpegAudioResolver::open(source).expect("open");
        let mut readers = starts.map(|_| AudioRangeReader::open(source).expect("open reader"));
        for round in 0..4 {
            for (start, reader) in starts.iter().zip(&mut readers) {
                let start = start + (round * CHUNK) as u64;
                let expected = reader.read(start, CHUNK).expect("read");
                let range = pooled.resolve_range(start, CHUNK).expect("resolve");
                let channel = |channel: usize| -> Vec<f32> {
                    expected.interleaved_f32[channel..]
                        .iter()
                        .step_by(2)
                        .copied()
                        .collect()
                };
                assert_eq!(range.channels()[0], channel(0), "left from {start}");
                assert_eq!(range.channels()[1], channel(1), "right from {start}");
            }
        }
        let _ = std::fs::remove_file(path);
    }
}
