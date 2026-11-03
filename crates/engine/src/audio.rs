//! Audio timeline, source, and mixing primitives.

use std::{collections::HashSet, sync::Arc};

use serde::{Deserialize, Serialize};

use crate::{error::MediaError, media::MediaStore};

pub const AUDIO_SAMPLE_RATE: u32 = 48_000;
pub const AUDIO_CHANNELS: usize = 2;
const CLIP_EDGE_FADE_SAMPLES: u64 = 384;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioTimeline {
    pub tracks: Vec<AudioTrack>,
    pub clips: Vec<AudioClip>,
}

impl AudioTimeline {
    /// What stops the timeline from playing as written: repeated ids, clips on tracks it doesn't
    /// declare, gains that aren't finite and at least 0, and clips without a duration.
    pub(crate) fn problems(&self) -> Vec<String> {
        let mut problems = Vec::new();
        let mut tracks = HashSet::new();
        for track in &self.tracks {
            if !tracks.insert(track.id.as_str()) {
                problems.push(format!("duplicate track id `{}`", track.id));
            }
            if !is_valid_gain(track.volume) {
                problems.push(format!(
                    "track `{}` volume must be a finite number of at least 0",
                    track.id
                ));
            }
        }
        let declared = match self.tracks.as_slice() {
            [] => "none".to_string(),
            tracks => tracks
                .iter()
                .map(|track| format!("`{}`", track.id))
                .collect::<Vec<_>>()
                .join(", "),
        };
        let mut clips = HashSet::new();
        for clip in &self.clips {
            if !clips.insert(clip.id.as_str()) {
                problems.push(format!("duplicate clip id `{}`", clip.id));
            }
            if !tracks.contains(clip.track_id.as_str()) {
                problems.push(format!(
                    "clip `{}` names unknown track `{}`; declared tracks: {declared}",
                    clip.id, clip.track_id
                ));
            }
            if !is_valid_gain(clip.volume) {
                problems.push(format!(
                    "clip `{}` volume must be a finite number of at least 0",
                    clip.id
                ));
            }
            if clip.duration_ms == 0 {
                problems.push(format!(
                    "clip `{}` duration must be greater than zero",
                    clip.id
                ));
            }
        }
        problems
    }
}

/// Negative zero is what a tiny negative volume in a document narrows to, so it counts as negative.
fn is_valid_gain(gain: f32) -> bool {
    gain.is_finite() && gain.is_sign_positive()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all(serialize = "camelCase"))]
pub struct AudioTrack {
    pub id: String,
    #[serde(default)]
    pub muted: bool,
    #[serde(default)]
    pub solo: bool,
    #[serde(default = "default_volume")]
    pub volume: f32,
}

pub(crate) fn default_volume() -> f32 {
    1.0
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioClip {
    pub id: String,
    pub source_id: String,
    pub track_id: String,
    pub start_ms: u64,
    pub duration_ms: u64,
    pub source_start_ms: u64,
    pub volume: f32,
}

/// Samples at [`AUDIO_SAMPLE_RATE`], one list per channel, all the same length.
#[derive(Debug, Clone, PartialEq)]
pub struct AudioBuffer {
    channels: Vec<Vec<f32>>,
}

impl AudioBuffer {
    /// Panics if the channels differ in length.
    pub fn from_channels(channels: Vec<Vec<f32>>) -> Self {
        if let Some(first) = channels.first() {
            assert!(
                channels.iter().all(|channel| channel.len() == first.len()),
                "audio channels must be the same length"
            );
        }
        Self { channels }
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
    /// `frames` frames from `start_sample`, at [`AUDIO_SAMPLE_RATE`]. A source that ends early
    /// returns fewer, or pads with silence.
    fn resolve_range(
        &self,
        start_sample: u64,
        frames: usize,
    ) -> Result<Arc<AudioBuffer>, MediaError>;
}

pub struct AudioMixer<'a, P: MediaStore> {
    timeline: &'a AudioTimeline,
    provider: &'a P,
}

impl<'a, P: MediaStore> AudioMixer<'a, P> {
    pub fn new(timeline: &'a AudioTimeline, provider: &'a P) -> Self {
        Self { timeline, provider }
    }

    pub fn mix_range(&self, start_sample: u64, frames: usize) -> Result<AudioBuffer, MediaError> {
        let mut output = vec![vec![0.0; frames]; AUDIO_CHANNELS];
        let has_solo = self.timeline.tracks.iter().any(|track| track.solo);

        for clip in &self.timeline.clips {
            let track = self
                .timeline
                .tracks
                .iter()
                .find(|track| track.id == clip.track_id)
                .ok_or_else(|| MediaError::UnknownAudioTrack {
                    clip: clip.id.clone(),
                    track: clip.track_id.clone(),
                })?;
            if track.muted || (has_solo && !track.solo) || track.volume <= 0.0 || clip.volume <= 0.0
            {
                continue;
            }

            let clip_start = ms_to_sample(clip.start_ms);
            let clip_duration = ms_to_sample(clip.duration_ms).max(1);
            let clip_end = clip_start.saturating_add(clip_duration);
            let output_end = start_sample.saturating_add(frames as u64);
            let overlap_start = start_sample.max(clip_start);
            let overlap_end = output_end.min(clip_end);
            if overlap_end <= overlap_start {
                continue;
            }

            // Both lie within the output's `frames`.
            let frames_to_mix = (overlap_end - overlap_start) as usize;
            let output_offset = (overlap_start - start_sample) as usize;
            let clip_offset = overlap_start - clip_start;
            let source_start = ms_to_sample(clip.source_start_ms).saturating_add(clip_offset);
            let source = self.provider.get_audio_resolver(&clip.source_id)?;
            let source_buffer = source.resolve_range(source_start, frames_to_mix)?;
            let gain = track.volume * clip.volume;

            for i in 0..frames_to_mix.min(source_buffer.frames()) {
                let clip_sample = clip_offset.saturating_add(i as u64);
                let sample_gain = gain * clip_envelope(clip_sample, clip_duration);
                for (channel, output_channel) in output.iter_mut().enumerate() {
                    let source_channel = channel.min(source_buffer.channel_count() - 1);
                    output_channel[output_offset + i] +=
                        source_buffer.channels()[source_channel][i] * sample_gain;
                }
            }
        }

        for sample in output.iter_mut().flatten() {
            *sample = sample.clamp(-1.0, 1.0);
        }

        Ok(AudioBuffer::from_channels(output))
    }
}

pub fn ms_to_sample(ms: u64) -> u64 {
    ((ms as u128 * u128::from(AUDIO_SAMPLE_RATE)) / 1_000) as u64
}

pub fn duration_samples(duration_frames: u32, fps: f32) -> u64 {
    ((duration_frames as f64 / fps as f64) * f64::from(AUDIO_SAMPLE_RATE)).round() as u64
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
    use std::collections::HashMap;

    use super::*;

    #[derive(Debug)]
    struct TestProvider {
        sources: HashMap<String, Arc<AudioBuffer>>,
    }

    impl crate::media::MediaStore for TestProvider {
        fn get_audio_resolver(
            &self,
            source_id: &str,
        ) -> Result<Box<dyn AudioResolver>, crate::error::MediaError> {
            self.sources
                .get(source_id)
                .cloned()
                .map(|buffer| Box::new(TestResolver { buffer }) as Box<dyn AudioResolver>)
                .ok_or_else(|| {
                    crate::error::MediaError::source_not_found(source_id, "not registered")
                })
        }
    }

    struct TestResolver {
        buffer: Arc<AudioBuffer>,
    }

    impl AudioResolver for TestResolver {
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
            Ok(Arc::new(AudioBuffer::from_channels(channels)))
        }
    }

    fn track(id: &str) -> AudioTrack {
        AudioTrack {
            id: id.to_string(),
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
                Arc::new(AudioBuffer::from_channels(vec![vec![
                    0.5;
                    AUDIO_SAMPLE_RATE
                        as usize
                ]])),
            )]),
        };
        let timeline = AudioTimeline {
            tracks: vec![track("main")],
            clips: vec![AudioClip {
                id: "clip".to_string(),
                source_id: "tone".to_string(),
                track_id: "main".to_string(),
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
                Arc::new(AudioBuffer::from_channels(vec![
                    vec![1.0; AUDIO_SAMPLE_RATE as usize],
                    vec![0.5; AUDIO_SAMPLE_RATE as usize],
                ])),
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
                    start_ms: 0,
                    duration_ms: 1_000,
                    source_start_ms: 0,
                    volume: 0.5,
                },
                AudioClip {
                    id: "b".to_string(),
                    source_id: "tone".to_string(),
                    track_id: "muted".to_string(),
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
    fn a_clip_on_an_undeclared_track_is_an_error_rather_than_silence() {
        let provider = TestProvider {
            sources: HashMap::new(),
        };
        let timeline = AudioTimeline {
            tracks: vec![track("main")],
            clips: vec![AudioClip {
                id: "clip".to_string(),
                source_id: "tone".to_string(),
                track_id: "typo".to_string(),
                start_ms: 0,
                duration_ms: 1_000,
                source_start_ms: 0,
                volume: 1.0,
            }],
        };

        let error = AudioMixer::new(&timeline, &provider)
            .mix_range(0, 4)
            .unwrap_err();
        assert!(matches!(error, MediaError::UnknownAudioTrack { .. }));
    }

    #[test]
    fn overlaps_clips_and_clamps_output() {
        let provider = TestProvider {
            sources: HashMap::from([(
                "tone".to_string(),
                Arc::new(AudioBuffer::from_channels(vec![vec![
                    0.75;
                    AUDIO_SAMPLE_RATE
                        as usize
                ]])),
            )]),
        };
        let timeline = AudioTimeline {
            tracks: vec![track("main")],
            clips: vec![
                AudioClip {
                    id: "a".to_string(),
                    source_id: "tone".to_string(),
                    track_id: "main".to_string(),
                    start_ms: 0,
                    duration_ms: 1_000,
                    source_start_ms: 0,
                    volume: 1.0,
                },
                AudioClip {
                    id: "b".to_string(),
                    source_id: "tone".to_string(),
                    track_id: "main".to_string(),
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

    /// One repeating stereo pattern per source, so a wrong source offset changes the samples.
    #[derive(Serialize)]
    struct FixtureSource {
        pattern: Vec<[f32; 2]>,
        frames: usize,
    }

    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct FixtureTimeline {
        #[serde(flatten)]
        timeline: AudioTimeline,
        duration_frames: u32,
        fps: u32,
    }

    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct MixFixture {
        name: &'static str,
        timeline: FixtureTimeline,
        sources: HashMap<String, FixtureSource>,
        start_sample: u64,
        frames: usize,
        expected: Vec<f32>,
    }

    fn fixture_clip(id: &str, track_id: &str, start_ms: u64, duration_ms: u64) -> AudioClip {
        AudioClip {
            id: id.to_string(),
            source_id: "tone".to_string(),
            track_id: track_id.to_string(),
            start_ms,
            duration_ms,
            source_start_ms: 0,
            volume: 1.0,
        }
    }

    fn mix_fixture(
        name: &'static str,
        tracks: Vec<AudioTrack>,
        clips: Vec<AudioClip>,
        sources: Vec<(&str, Vec<[f32; 2]>, usize)>,
        start_sample: u64,
        frames: usize,
    ) -> MixFixture {
        let timeline = AudioTimeline { tracks, clips };
        let provider = TestProvider {
            sources: sources
                .iter()
                .map(|(id, pattern, frames)| {
                    let channel = |index: usize| {
                        (0..*frames)
                            .map(|frame| pattern[frame % pattern.len()][index])
                            .collect::<Vec<_>>()
                    };
                    let buffer = AudioBuffer::from_channels(vec![channel(0), channel(1)]);
                    (id.to_string(), Arc::new(buffer))
                })
                .collect(),
        };
        let expected = AudioMixer::new(&timeline, &provider)
            .mix_range(start_sample, frames)
            .unwrap()
            .interleaved_f32();
        MixFixture {
            name,
            timeline: FixtureTimeline {
                timeline,
                duration_frames: 60,
                fps: 1,
            },
            sources: sources
                .into_iter()
                .map(|(id, pattern, frames)| (id.to_string(), FixtureSource { pattern, frames }))
                .collect(),
            start_sample,
            frames,
            expected,
        }
    }

    fn mix_fixtures() -> Vec<MixFixture> {
        let pattern = vec![
            [0.5, -0.25],
            [0.25, 0.75],
            [-0.5, 0.125],
            [0.75, -0.75],
            [-0.125, 0.5],
            [0.375, -0.375],
            [-0.75, 0.25],
        ];
        let tone = || vec![("tone", pattern.clone(), 96_000)];
        let main = || vec![track("main")];
        let long_clip = || fixture_clip("clip", "main", 100, 1_000);

        let mut offset = long_clip();
        offset.source_start_ms = 2;
        let mut quiet_track = track("main");
        quiet_track.volume = 0.5;
        let mut quiet_clip = long_clip();
        quiet_clip.volume = 0.25;
        let mut solo = track("solo");
        solo.solo = true;
        let mut muted = track("muted");
        muted.muted = true;
        let mut silent_track = track("main");
        silent_track.volume = 0.0;
        let overlap = |id: &str, source: &str| AudioClip {
            source_id: source.to_string(),
            ..fixture_clip(id, "main", 0, 1_000)
        };
        let overlap_sources = || {
            vec![
                ("up", vec![[0.8, -0.8]], 96_000),
                ("down", vec![[-0.8, 0.8]], 96_000),
            ]
        };

        vec![
            mix_fixture(
                "start position and source offset",
                main(),
                vec![offset],
                tone(),
                6_000,
                16,
            ),
            mix_fixture(
                "track and clip gain multiply",
                vec![quiet_track],
                vec![quiet_clip],
                tone(),
                6_000,
                16,
            ),
            mix_fixture(
                "fades in over 384 samples from the clip start",
                main(),
                vec![long_clip()],
                tone(),
                4_796,
                16,
            ),
            mix_fixture(
                "fade-in reaches full gain",
                main(),
                vec![long_clip()],
                tone(),
                5_176,
                16,
            ),
            mix_fixture(
                "fades out over the last 384 samples",
                main(),
                vec![long_clip()],
                tone(),
                52_416,
                16,
            ),
            mix_fixture(
                "reaches silence at the clip end",
                main(),
                vec![long_clip()],
                tone(),
                52_792,
                16,
            ),
            mix_fixture(
                "a short clip fades over half its length",
                main(),
                vec![fixture_clip("clip", "main", 0, 10)],
                tone(),
                232,
                16,
            ),
            mix_fixture(
                "a very short clip fades over half its length",
                main(),
                vec![fixture_clip("clip", "main", 0, 1)],
                tone(),
                16,
                40,
            ),
            mix_fixture(
                "a solo track silences the others",
                vec![solo, track("other")],
                vec![
                    fixture_clip("kept", "solo", 0, 1_000),
                    fixture_clip("dropped", "other", 0, 1_000),
                ],
                tone(),
                1_000,
                16,
            ),
            mix_fixture(
                "a muted track is silent",
                vec![muted, track("audible")],
                vec![
                    fixture_clip("dropped", "muted", 0, 1_000),
                    fixture_clip("kept", "audible", 0, 1_000),
                ],
                tone(),
                1_000,
                16,
            ),
            mix_fixture(
                "a silent track adds nothing",
                vec![silent_track],
                vec![long_clip()],
                tone(),
                6_000,
                16,
            ),
            mix_fixture(
                "overlaps sum before clamping once",
                main(),
                vec![overlap("a", "up"), overlap("b", "up"), overlap("c", "down")],
                overlap_sources(),
                1_000,
                4,
            ),
            mix_fixture(
                "the mix clamps in both directions",
                main(),
                vec![overlap("a", "up"), overlap("b", "up")],
                overlap_sources(),
                1_000,
                4,
            ),
            mix_fixture(
                "a source shorter than its clip ends in silence",
                main(),
                vec![fixture_clip("clip", "main", 0, 1_000)],
                vec![("tone", pattern.clone(), 1_000)],
                992,
                16,
            ),
            mix_fixture(
                "a window can span the gap between two clips",
                main(),
                vec![
                    fixture_clip("first", "main", 0, 10),
                    fixture_clip("second", "main", 11, 10),
                ],
                tone(),
                470,
                80,
            ),
        ]
    }

    /// `packages/lumen-preview/src/audio-worker.test.ts` mixes the same file in the browser mixer,
    /// so a change on either side fails until both agree.
    #[test]
    fn browser_mixer_fixtures_match_this_mixer() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/audio_mix.json");
        let generated = serde_json::to_string_pretty(&mix_fixtures()).unwrap();
        if std::env::var_os("LUMEN_BLESS_AUDIO_FIXTURES").is_some() {
            std::fs::write(path, format!("{generated}\n")).unwrap();
        }
        // Parsed so the committed file's formatting doesn't matter.
        let parse = |text: &str| serde_json::from_str::<serde_json::Value>(text).unwrap();
        let committed = parse(&std::fs::read_to_string(path).unwrap());
        let generated = parse(&generated);
        assert_eq!(
            generated, committed,
            "the mixer changed: rerun with LUMEN_BLESS_AUDIO_FIXTURES=1, then update the browser mixer to match"
        );
    }
}
