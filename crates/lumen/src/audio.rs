//! Audio timeline, source, and mixing primitives.

use std::{collections::HashMap, sync::Arc};

use crate::error::MediaError;

pub const AUDIO_SAMPLE_RATE: u32 = 48_000;
pub const AUDIO_CHANNELS: usize = 2;
const CLIP_EDGE_FADE_SAMPLES: u64 = 384;

#[derive(Debug, Clone, PartialEq)]
pub struct AudioTimeline {
    pub tracks: Vec<AudioTrack>,
    pub clips: Vec<AudioClip>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AudioTrack {
    pub id: String,
    pub name: String,
    pub muted: bool,
    pub solo: bool,
    pub volume: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AudioClip {
    pub id: String,
    pub source_id: String,
    pub track_id: String,
    pub name: String,
    pub start_ms: u64,
    pub duration_ms: u64,
    pub source_start_ms: u64,
    pub volume: f32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct AudioMetadata {
    pub sample_rate: u32,
    pub channels: u16,
    pub duration_samples: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AudioBuffer {
    channels: Vec<Vec<f32>>,
    sample_rate: u32,
}

impl AudioBuffer {
    pub fn silent(sample_rate: u32, channel_count: usize, frames: usize) -> Self {
        Self {
            channels: vec![vec![0.0; frames]; channel_count],
            sample_rate,
        }
    }

    pub fn from_channels(sample_rate: u32, channels: Vec<Vec<f32>>) -> Self {
        Self {
            sample_rate,
            channels,
        }
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn channel_count(&self) -> usize {
        self.channels.len()
    }

    pub fn frames(&self) -> usize {
        self.channels.first().map_or(0, Vec::len)
    }

    pub fn channels(&self) -> &[Vec<f32>] {
        &self.channels
    }

    pub fn channels_mut(&mut self) -> &mut [Vec<f32>] {
        &mut self.channels
    }

    pub fn interleaved_f32(&self) -> Vec<f32> {
        let frames = self.frames();
        let channels = self.channel_count();
        let mut output = Vec::with_capacity(frames.saturating_mul(channels));
        for frame in 0..frames {
            for channel in 0..channels {
                output.push(self.channels[channel][frame]);
            }
        }
        output
    }
}

pub trait AudioResolver: Send + Sync {
    fn id(&self) -> &str;

    fn metadata(&self) -> AudioMetadata;

    fn resolve_range(
        &self,
        start_sample: u64,
        frames: usize,
    ) -> Result<Arc<AudioBuffer>, MediaError>;
}

pub trait AudioSourceProvider {
    fn get_audio_resolver(&self, source_id: &str) -> Option<Box<dyn AudioResolver>>;
}

pub struct AudioMixer<'a, P: AudioSourceProvider> {
    timeline: &'a AudioTimeline,
    provider: &'a P,
}

impl<'a, P: AudioSourceProvider> AudioMixer<'a, P> {
    pub fn new(timeline: &'a AudioTimeline, provider: &'a P) -> Self {
        Self { timeline, provider }
    }

    pub fn mix_range(&self, start_sample: u64, frames: usize) -> Result<AudioBuffer, MediaError> {
        let mut output = AudioBuffer::silent(AUDIO_SAMPLE_RATE, AUDIO_CHANNELS, frames);
        self.mix_into(start_sample, output.channels_mut())?;
        Ok(output)
    }

    pub fn mix_into(&self, start_sample: u64, output: &mut [Vec<f32>]) -> Result<(), MediaError> {
        let output_frames = output.first().map_or(0, Vec::len);
        if output_frames == 0 || output.is_empty() {
            return Ok(());
        }

        for channel in output.iter_mut() {
            channel.fill(0.0);
        }

        let has_solo = self.timeline.tracks.iter().any(|track| track.solo);
        let tracks = self
            .timeline
            .tracks
            .iter()
            .map(|track| (track.id.as_str(), track))
            .collect::<HashMap<_, _>>();

        for clip in &self.timeline.clips {
            let Some(track) = tracks.get(clip.track_id.as_str()) else {
                continue;
            };
            if track.muted || (has_solo && !track.solo) || track.volume <= 0.0 || clip.volume <= 0.0
            {
                continue;
            }

            let clip_start = ms_to_sample(clip.start_ms);
            let clip_duration = ms_to_sample(clip.duration_ms).max(1);
            let clip_end = clip_start.saturating_add(clip_duration);
            let output_end = start_sample.saturating_add(output_frames as u64);
            let overlap_start = start_sample.max(clip_start);
            let overlap_end = output_end.min(clip_end);
            if overlap_end <= overlap_start {
                continue;
            }

            let frames_to_mix = usize::try_from(overlap_end - overlap_start).unwrap_or(0);
            let output_offset = usize::try_from(overlap_start - start_sample).unwrap_or(0);
            let clip_offset = overlap_start - clip_start;
            let source_start = ms_to_sample(clip.source_start_ms).saturating_add(clip_offset);
            let source = self
                .provider
                .get_audio_resolver(&clip.source_id)
                .ok_or_else(|| MediaError::SourceNotFound {
                    media_source: clip.source_id.clone(),
                })?;
            let source_buffer = source.resolve_range(source_start, frames_to_mix)?;
            let gain = track.volume * clip.volume;

            for i in 0..frames_to_mix.min(source_buffer.frames()) {
                let clip_sample = clip_offset.saturating_add(i as u64);
                let envelope = clip_envelope(clip_sample, clip_duration);
                let sample_gain = gain * envelope;
                for (out_channel_index, output_channel) in output.iter_mut().enumerate() {
                    let source_sample =
                        sample_for_output_channel(&source_buffer, out_channel_index, i);
                    let Some(output_sample) = output_channel.get_mut(output_offset + i) else {
                        continue;
                    };
                    *output_sample += source_sample * sample_gain;
                }
            }
        }

        for channel in output {
            for sample in channel {
                *sample = sample.clamp(-1.0, 1.0);
            }
        }

        Ok(())
    }
}

pub fn ms_to_sample(ms: u64) -> u64 {
    ((ms as u128 * u128::from(AUDIO_SAMPLE_RATE)) / 1_000) as u64
}

pub fn duration_samples(duration_frames: u32, fps: f32) -> u64 {
    if fps <= 0.0 {
        return 0;
    }
    ((duration_frames as f64 / fps as f64) * f64::from(AUDIO_SAMPLE_RATE)).round() as u64
}

fn sample_for_output_channel(buffer: &AudioBuffer, output_channel: usize, frame: usize) -> f32 {
    match buffer.channel_count() {
        0 => 0.0,
        1 => buffer.channels()[0].get(frame).copied().unwrap_or(0.0),
        count => buffer.channels()[output_channel.min(count - 1)]
            .get(frame)
            .copied()
            .unwrap_or(0.0),
    }
}

fn clip_envelope(clip_sample: u64, clip_duration: u64) -> f32 {
    let fade = CLIP_EDGE_FADE_SAMPLES.min(clip_duration / 2);
    if fade == 0 {
        return 1.0;
    }
    if clip_sample < fade {
        return clip_sample as f32 / fade as f32;
    }
    let remaining = clip_duration.saturating_sub(clip_sample);
    if remaining < fade {
        return remaining as f32 / fade as f32;
    }
    1.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct TestProvider {
        sources: HashMap<String, Arc<AudioBuffer>>,
    }

    impl AudioSourceProvider for TestProvider {
        fn get_audio_resolver(&self, source_id: &str) -> Option<Box<dyn AudioResolver>> {
            self.sources.get(source_id).cloned().map(|buffer| {
                Box::new(TestResolver {
                    id: source_id.to_string(),
                    buffer,
                }) as Box<dyn AudioResolver>
            })
        }
    }

    struct TestResolver {
        id: String,
        buffer: Arc<AudioBuffer>,
    }

    impl AudioResolver for TestResolver {
        fn id(&self) -> &str {
            &self.id
        }

        fn metadata(&self) -> AudioMetadata {
            AudioMetadata {
                sample_rate: self.buffer.sample_rate(),
                channels: self.buffer.channel_count() as u16,
                duration_samples: self.buffer.frames() as u64,
            }
        }

        fn resolve_range(
            &self,
            start_sample: u64,
            frames: usize,
        ) -> Result<Arc<AudioBuffer>, MediaError> {
            let start = usize::try_from(start_sample).unwrap_or(usize::MAX);
            let end = start.saturating_add(frames).min(self.buffer.frames());
            let mut channels = Vec::new();
            for channel in self.buffer.channels() {
                let mut out = vec![0.0; frames];
                if start < channel.len() {
                    let src = &channel[start..end];
                    out[..src.len()].copy_from_slice(src);
                }
                channels.push(out);
            }
            Ok(Arc::new(AudioBuffer::from_channels(
                self.buffer.sample_rate(),
                channels,
            )))
        }
    }

    fn track(id: &str) -> AudioTrack {
        AudioTrack {
            id: id.to_string(),
            name: id.to_string(),
            muted: false,
            solo: false,
            volume: 1.0,
        }
    }

    #[test]
    fn mixes_single_clip_with_source_offset() {
        let provider = TestProvider {
            sources: HashMap::from([(
                "tone".to_string(),
                Arc::new(AudioBuffer::from_channels(
                    AUDIO_SAMPLE_RATE,
                    vec![vec![0.5; AUDIO_SAMPLE_RATE as usize]],
                )),
            )]),
        };
        let timeline = AudioTimeline {
            tracks: vec![track("main")],
            clips: vec![AudioClip {
                id: "clip".to_string(),
                source_id: "tone".to_string(),
                track_id: "main".to_string(),
                name: "tone".to_string(),
                start_ms: 0,
                duration_ms: 1_000,
                source_start_ms: 0,
                volume: 1.0,
            }],
        };

        let mixed = AudioMixer::new(&timeline, &provider)
            .mix_range(ms_to_sample(500), 5)
            .unwrap();
        assert_eq!(mixed.channel_count(), 2);
        assert!(mixed.channels()[0][2] > 0.49);
        assert_eq!(mixed.channels()[0], mixed.channels()[1]);
    }

    #[test]
    fn applies_mute_solo_and_gain() {
        let provider = TestProvider {
            sources: HashMap::from([(
                "tone".to_string(),
                Arc::new(AudioBuffer::from_channels(
                    AUDIO_SAMPLE_RATE,
                    vec![
                        vec![1.0; AUDIO_SAMPLE_RATE as usize],
                        vec![0.5; AUDIO_SAMPLE_RATE as usize],
                    ],
                )),
            )]),
        };
        let mut solo = track("solo");
        solo.solo = true;
        solo.volume = 0.5;
        let mut muted = track("muted");
        muted.muted = true;
        let timeline = AudioTimeline {
            tracks: vec![solo, muted],
            clips: vec![
                AudioClip {
                    id: "a".to_string(),
                    source_id: "tone".to_string(),
                    track_id: "solo".to_string(),
                    name: "a".to_string(),
                    start_ms: 0,
                    duration_ms: 1_000,
                    source_start_ms: 0,
                    volume: 0.5,
                },
                AudioClip {
                    id: "b".to_string(),
                    source_id: "tone".to_string(),
                    track_id: "muted".to_string(),
                    name: "b".to_string(),
                    start_ms: 0,
                    duration_ms: 1_000,
                    source_start_ms: 0,
                    volume: 1.0,
                },
            ],
        };

        let mixed = AudioMixer::new(&timeline, &provider)
            .mix_range(500, 4)
            .unwrap();
        assert_eq!(mixed.channels()[0], vec![0.25; 4]);
        assert_eq!(mixed.channels()[1], vec![0.125; 4]);
    }

    #[test]
    fn overlaps_clips_and_clamps_output() {
        let provider = TestProvider {
            sources: HashMap::from([(
                "tone".to_string(),
                Arc::new(AudioBuffer::from_channels(
                    AUDIO_SAMPLE_RATE,
                    vec![vec![0.75; AUDIO_SAMPLE_RATE as usize]],
                )),
            )]),
        };
        let timeline = AudioTimeline {
            tracks: vec![track("main")],
            clips: vec![
                AudioClip {
                    id: "a".to_string(),
                    source_id: "tone".to_string(),
                    track_id: "main".to_string(),
                    name: "a".to_string(),
                    start_ms: 0,
                    duration_ms: 1_000,
                    source_start_ms: 0,
                    volume: 1.0,
                },
                AudioClip {
                    id: "b".to_string(),
                    source_id: "tone".to_string(),
                    track_id: "main".to_string(),
                    name: "b".to_string(),
                    start_ms: 0,
                    duration_ms: 1_000,
                    source_start_ms: 0,
                    volume: 1.0,
                },
            ],
        };

        let mixed = AudioMixer::new(&timeline, &provider)
            .mix_range(500, 4)
            .unwrap();
        assert_eq!(mixed.channels()[0], vec![1.0; 4]);
        assert_eq!(mixed.channels()[1], vec![1.0; 4]);
    }
}
