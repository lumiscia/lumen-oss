use std::{
    collections::HashMap,
    f64::consts::FRAC_PI_2,
    sync::{
        Arc, RwLock,
        atomic::{AtomicUsize, Ordering},
    },
};

use serde::Serialize;
use serde_json::{Value, json};

use super::*;
use crate::{composition::Composition, error::MediaError, media::MediaIdentity};

#[derive(Debug, Default)]
struct TestProvider {
    sources: RwLock<HashMap<String, (MediaIdentity, Arc<AudioBuffer>)>>,
    /// How many ranges were read from any source.
    reads: Arc<AtomicUsize>,
}

impl TestProvider {
    fn with(sources: &[(&str, AudioBuffer)]) -> Self {
        let provider = Self::default();
        for (source, buffer) in sources {
            provider.set(source, source, buffer.clone());
        }
        provider
    }

    /// Serves `buffer` as `source`, under the identity keyed `id`.
    fn set(&self, source: &str, id: &str, buffer: AudioBuffer) {
        let served = (MediaIdentity::of_key(id), Arc::new(buffer));
        self.sources
            .write()
            .unwrap()
            .insert(source.to_string(), served);
    }
}

impl crate::media::MediaStore for TestProvider {
    fn resolve(
        &self,
        source: &str,
        _: crate::media::AssetKind,
    ) -> Result<crate::media::ResolvedMedia, MediaError> {
        let (identity, buffer) = self
            .sources
            .read()
            .unwrap()
            .get(source)
            .cloned()
            .ok_or_else(|| MediaError::source_not_found(source, "not registered"))?;
        let reads = Arc::clone(&self.reads);
        Ok(crate::media::ResolvedMedia::new(
            identity,
            crate::media::MediaResolver::Audio(Box::new(TestResolver { buffer, reads })),
        ))
    }
}

struct TestResolver {
    buffer: Arc<AudioBuffer>,
    reads: Arc<AtomicUsize>,
}

impl AudioResolver for TestResolver {
    fn resolve_range(
        &self,
        start_sample: u64,
        frames: usize,
    ) -> Result<Arc<AudioBuffer>, MediaError> {
        self.reads.fetch_add(1, Ordering::Relaxed);
        let start = usize::try_from(start_sample).unwrap_or(usize::MAX);
        let end = start.saturating_add(frames).min(self.buffer.frames());
        let channels = self
            .buffer
            .channels()
            .iter()
            .map(|channel| {
                let mut out = vec![0.0; frames];
                if start < channel.len() {
                    out[..end - start].copy_from_slice(&channel[start..end]);
                }
                out
            })
            .collect();
        Ok(Arc::new(AudioBuffer::from_channels(channels)))
    }
}

const RATE: usize = AUDIO_SAMPLE_RATE as usize;

/// A mono source holding `value` for `seconds`.
fn constant(value: f32, seconds: f64) -> AudioBuffer {
    AudioBuffer::from_channels(vec![vec![value; (seconds * RATE as f64) as usize]])
}

/// A mono source whose sample `n` is `n / RATE`, so the output says which source sample played.
fn ramp(seconds: f64) -> AudioBuffer {
    let frames = (seconds * RATE as f64) as usize;
    AudioBuffer::from_channels(vec![(0..frames).map(|n| n as f32 / RATE as f32).collect()])
}

fn audio_clip(id: &str, source: &str, start_ms: u64, duration_ms: u64, extra: Value) -> Value {
    let mut clip = json!({
        "id": id, "source_id": source, "track_id": "main",
        "start_ms": start_ms, "duration_ms": duration_ms
    });
    clip.as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    clip
}

fn audio(clips: Vec<Value>) -> Value {
    json!({ "tracks": [{ "id": "main" }], "clips": clips })
}

/// Nodes and connections that show `layers` merged in order, or a background when there are none.
fn graph(layers: Vec<Value>) -> (Value, Value) {
    let mut nodes = vec![json!({ "id": 1, "type": "background" })];
    let mut connections = Vec::new();
    let mut shown = 1;
    for (index, layer) in layers.into_iter().enumerate() {
        let id = 10 + 2 * index as u64;
        let mut layer = layer;
        layer["id"] = json!(id);
        nodes.push(layer);
        nodes.push(json!({ "id": id + 1, "type": "merge" }));
        connections.push(json!({ "from_node": shown, "to_node": id + 1, "to_port": "base" }));
        connections.push(json!({ "from_node": id, "to_node": id + 1, "to_port": "overlay" }));
        shown = id + 1;
    }
    nodes.push(json!({ "id": 2, "type": "media_output" }));
    connections.push(json!({ "from_node": shown, "to_node": 2, "to_port": "source" }));
    (Value::from(nodes), Value::from(connections))
}

fn clip_node(params: Value) -> Value {
    json!({ "type": "clip", "params": params })
}

fn template(fps: f64, frames: u32, audio: Option<Value>, layers: Vec<Value>) -> Value {
    let (nodes, connections) = graph(layers);
    let mut template = json!({
        "timeline": { "fps": fps, "duration_frames": frames },
        "nodes": nodes,
        "connections": connections
    });
    if let Some(audio) = audio {
        template["audio"] = audio;
    }
    template
}

fn document(
    fps: f64,
    frames: u32,
    audio: Option<Value>,
    compositions: Value,
    layers: Vec<Value>,
) -> Value {
    let (nodes, connections) = graph(layers);
    let mut document = json!({
        "timeline": { "fps": fps, "duration_frames": frames },
        "render_settings": { "width": 4, "height": 4 },
        "compositions": compositions,
        "nodes": nodes,
        "connections": connections
    });
    if let Some(audio) = audio {
        document["audio"] = audio;
    }
    document
}

fn composition(document: &Value) -> Composition {
    crate::json::parse_value(document).unwrap_or_else(|error| panic!("{error}"))
}

/// The left channel of root samples `[start, start + frames)`, mixed `chunk` samples at a time.
fn mix(
    composition: &Composition,
    provider: &TestProvider,
    start: u64,
    frames: usize,
    chunk: usize,
) -> Vec<f32> {
    let mut mixer = AudioMixer::new(composition, provider);
    let mut left = Vec::with_capacity(frames);
    let mut at = 0;
    while at < frames {
        let count = chunk.min(frames - at);
        let mixed = mixer.mix_range(start + at as u64, count).unwrap();
        left.extend_from_slice(&mixed.channels()[0]);
        at += count;
    }
    left
}

fn seconds(sample: usize) -> f64 {
    sample as f64 / RATE as f64
}

/// The first and last samples louder than `threshold`.
fn audible_span(samples: &[f32], threshold: f32) -> Option<(usize, usize)> {
    let first = samples.iter().position(|sample| sample.abs() > threshold)?;
    let last = samples
        .iter()
        .rposition(|sample| sample.abs() > threshold)?;
    Some((first, last))
}

#[test]
fn a_source_with_a_new_identity_is_read_again_and_an_unchanged_one_is_not() {
    let provider = TestProvider::with(&[("tone", constant(0.5, 1.0))]);
    let document = document(
        30.0,
        30,
        Some(audio(vec![audio_clip("a", "tone", 0, 1_000, json!({}))])),
        json!({}),
        vec![],
    );
    let composition = composition(&document);
    let mut mixer = AudioMixer::new(&composition, &provider);
    let first = mixer.mix_range(1_000, 4).unwrap();
    let reads = provider.reads.load(Ordering::Relaxed);
    assert_eq!(
        mixer.mix_range(1_000, 4).unwrap().channels(),
        first.channels()
    );
    assert_eq!(
        provider.reads.load(Ordering::Relaxed),
        reads,
        "blocks are kept"
    );

    provider.set("tone", "louder", constant(0.25, 1.0));
    let replaced = mixer.mix_range(1_000, 4).unwrap();
    assert_eq!(replaced.channels()[0], vec![0.25; 4]);
    assert!(provider.reads.load(Ordering::Relaxed) > reads);
}

#[test]
fn mixes_stereo_with_mute_solo_and_gain() {
    let provider = TestProvider::with(&[(
        "tone",
        AudioBuffer::from_channels(vec![vec![1.0; RATE], vec![0.5; RATE]]),
    )]);
    let document = document(
        30.0,
        30,
        Some(json!({
            "tracks": [
                { "id": "solo", "solo": true, "volume": 0.5 },
                { "id": "muted", "muted": true }
            ],
            "clips": [
                { "id": "a", "source_id": "tone", "track_id": "solo", "start_ms": 0, "duration_ms": 1000, "volume": 0.5 },
                { "id": "b", "source_id": "tone", "track_id": "muted", "start_ms": 0, "duration_ms": 1000 }
            ]
        })),
        json!({}),
        vec![],
    );
    let composition = composition(&document);
    let mixed = AudioMixer::new(&composition, &provider)
        .mix_range(500, 4)
        .unwrap();
    assert_eq!(mixed.channels()[0], vec![0.25; 4]);
    assert_eq!(mixed.channels()[1], vec![0.125; 4]);
}

#[test]
fn an_animated_gain_follows_its_expression_without_steps() {
    let provider = TestProvider::with(&[("tone", constant(0.5, 3.0))]);
    let document = document(
        30.0,
        90,
        Some(audio(vec![audio_clip(
            "ramp",
            "tone",
            0,
            3_000,
            json!({ "volume": "=time / 2" }),
        )])),
        json!({}),
        vec![],
    );
    let composition = composition(&document);
    let whole = mix(&composition, &provider, 0, 2 * RATE, 2 * RATE);
    for sample in [1_000, 12_345, RATE, 90_000] {
        let expected = 0.5 * seconds(sample) / 2.0;
        assert!(
            (f64::from(whole[sample]) - expected).abs() < 1e-6,
            "at {sample}: {} vs {expected}",
            whole[sample]
        );
    }
    // Past the edge ramp, each sample rises by the same step: no jump at control points.
    let step = 0.5 / 2.0 / RATE as f64;
    for pair in whole[1_000..].windows(2) {
        assert!((f64::from(pair[1] - pair[0]) - step).abs() < 1e-6);
    }
    // Chunks of any size mix the same samples.
    assert_eq!(mix(&composition, &provider, 0, 2 * RATE, 777), whole);
}

#[test]
fn a_gain_can_duck_one_track_under_another() {
    let provider =
        TestProvider::with(&[("music", constant(0.4, 4.0)), ("voice", constant(0.2, 4.0))]);
    let document = document(
        30.0,
        120,
        Some(json!({
            "tracks": [
                { "id": "music", "volume": "=if(time >= 1 && time < 3, 0.25, 1)" },
                { "id": "voice" }
            ],
            "clips": [
                { "id": "bed", "source_id": "music", "track_id": "music", "start_ms": 0, "duration_ms": 4000 },
                { "id": "vo", "source_id": "voice", "track_id": "voice", "start_ms": 1000, "duration_ms": 2000 }
            ]
        })),
        json!({}),
        vec![],
    );
    let composition = composition(&document);
    let mixed = mix(&composition, &provider, 0, 4 * RATE, RATE);
    assert!((mixed[RATE / 2] - 0.4).abs() < 1e-6);
    assert!((mixed[2 * RATE] - (0.1 + 0.2)).abs() < 1e-6);
    assert!((mixed[7 * RATE / 2] - 0.4).abs() < 1e-6);
    // The switch moves over one control interval instead of stepping.
    let switch = &mixed[RATE - CONTROL_FRAMES as usize..RATE + 1];
    let biggest = switch
        .windows(2)
        .map(|pair| (pair[1] - pair[0]).abs())
        .fold(0.0, f32::max);
    assert!(biggest < 0.01, "{biggest}");
}

#[test]
fn fades_are_equal_power_and_exact_at_the_ends() {
    let provider = TestProvider::with(&[("tone", constant(1.0, 4.0))]);
    let document = document(
        30.0,
        120,
        Some(audio(vec![audio_clip(
            "faded",
            "tone",
            500,
            3_000,
            json!({ "fade_in_ms": 1000, "fade_out_ms": 1000 }),
        )])),
        json!({}),
        vec![],
    );
    let composition = composition(&document);
    let mixed = mix(&composition, &provider, 0, 4 * RATE, 4 * RATE);
    let start = RATE / 2;
    let end = start + 3 * RATE;
    let edge = |offset: usize| (offset as f64 / 384.0).min(1.0);
    let expected_in =
        |offset: usize| (offset as f64 / RATE as f64 * FRAC_PI_2).sin() * edge(offset);
    assert_eq!(mixed[start - 1], 0.0);
    assert_eq!(mixed[start], 0.0);
    for offset in [1, 383, 384, 1_000, RATE / 2, RATE - 1, RATE] {
        assert!(
            (f64::from(mixed[start + offset]) - expected_in(offset)).abs() < 1e-6,
            "fade in at {offset}"
        );
    }
    assert!((mixed[start + RATE / 2] - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6);
    assert!((mixed[start + 3 * RATE / 2] - 1.0).abs() < 1e-6);
    for remaining in [1, 383, 1_000, RATE / 2, RATE] {
        assert!(
            (f64::from(mixed[end - remaining]) - expected_in(remaining)).abs() < 1e-6,
            "fade out {remaining} before the end"
        );
    }
    assert_eq!(mixed[end], 0.0);
}

#[test]
fn fades_longer_than_a_short_clip_multiply_without_a_click() {
    let provider = TestProvider::with(&[("tone", constant(1.0, 1.0))]);
    let document = document(
        30.0,
        30,
        Some(audio(vec![audio_clip(
            "blip",
            "tone",
            0,
            10,
            json!({ "fade_in_ms": 1000, "fade_out_ms": 1000 }),
        )])),
        json!({}),
        vec![],
    );
    let composition = composition(&document);
    let mixed = mix(&composition, &provider, 0, 600, 600);
    let length = 480;
    assert!(mixed.iter().all(|sample| (0.0..=1.0).contains(sample)));
    assert!(mixed[length..].iter().all(|sample| *sample == 0.0));
    // Symmetric, rising to the middle and falling after it, so no sample jumps.
    for offset in 1..length / 2 {
        assert!(mixed[offset] >= mixed[offset - 1]);
        assert!((mixed[offset] - mixed[length - offset]).abs() < 1e-6);
    }
    assert!(mixed[length / 2] > 0.0);
}

/// A template that sounds `source` from 0.5 s to 0.75 s of its own time, 2 s long at `fps`.
fn burst_template(fps: f64, source: &str, extra: Value) -> Value {
    template(
        fps,
        (2.0 * fps) as u32,
        Some(audio(vec![audio_clip("burst", source, 500, 250, extra)])),
        vec![],
    )
}

fn burst_span(clip: Value, template_fps: f64, root_fps: f64) -> Option<(usize, usize)> {
    let provider = TestProvider::with(&[("tone", constant(1.0, 2.0))]);
    let document = document(
        root_fps,
        (6.0 * root_fps) as u32,
        None,
        json!({ "burst": burst_template(template_fps, "tone", json!({})) }),
        vec![clip_node(clip)],
    );
    let composition = composition(&document);
    let mixed = mix(&composition, &provider, 0, 6 * RATE, 6 * RATE);
    audible_span(&mixed, 1e-6)
}

#[test]
fn a_source_a_template_cant_play_names_the_template_and_clip() {
    let document = document(
        30.0,
        180,
        None,
        json!({ "burst": burst_template(30.0, "gone", json!({})) }),
        vec![clip_node(json!({ "composition": "burst" }))],
    );
    let composition = composition(&document);
    let provider = TestProvider::default();
    let error = AudioMixer::new(&composition, &provider)
        .mix_range(0, 6 * RATE)
        .unwrap_err();
    let failure = error.media_failure().expect("a media failure");
    assert_eq!(failure.code(), "media.not_found");
    assert_eq!(failure.source, "gone");
    assert_eq!(failure.wanted, [crate::media::AssetKind::Audio]);
    assert_eq!(
        failure.at,
        Some(crate::media::MediaLocation::AudioClip {
            template: Some("burst".to_string()),
            track: "main".to_string(),
            clip: "burst".to_string(),
            index: 0,
        })
    );
    let diagnostic = crate::diagnostic::Diagnostic::from_error(&error, &document);
    assert_eq!(diagnostic.code, "media.not_found");
    assert_eq!(
        diagnostic.path,
        "/compositions/burst/audio/clips/0/source_id"
    );
}

/// The place the mix of `document` fails at, when every source is missing.
fn failing_clip(document: &Value) -> crate::media::MediaLocation {
    let composition = composition(document);
    let provider = TestProvider::default();
    let error = AudioMixer::new(&composition, &provider)
        .mix_range(0, RATE)
        .unwrap_err();
    error
        .media_failure()
        .and_then(|failure| failure.at.clone())
        .unwrap_or_else(|| panic!("a located media failure: {error}"))
}

#[test]
fn a_failing_source_names_the_clip_that_plays_it_not_another_with_the_same_source() {
    use crate::media::MediaLocation::AudioClip;

    let muted = json!({ "id": "off", "muted": true });
    let tracks = json!([{ "id": "main" }, muted]);
    let clip = |id: &str, track: &str| json!({ "id": id, "source_id": "gone", "track_id": track, "start_ms": 0, "duration_ms": 500 });
    // A muted clip of the root comes first; the one that plays is the second.
    let root = json!({ "tracks": tracks, "clips": [clip("silent", "off"), clip("heard", "main")] });
    assert_eq!(
        failing_clip(&document(30.0, 60, Some(root), json!({}), vec![])),
        AudioClip {
            template: None,
            track: "main".to_string(),
            clip: "heard".to_string(),
            index: 1,
        }
    );
    // Another template, which nothing plays, has the same source in an earlier-sorting name.
    let unused = template(
        30.0,
        60,
        Some(audio(vec![audio_clip("other", "gone", 0, 500, json!({}))])),
        vec![],
    );
    let played = template(
        30.0,
        60,
        Some(audio(vec![audio_clip("burst", "gone", 0, 500, json!({}))])),
        vec![],
    );
    let document = document(
        30.0,
        60,
        None,
        json!({ "a_unused": unused, "b_played": played }),
        vec![clip_node(json!({ "composition": "b_played" }))],
    );
    assert_eq!(
        failing_clip(&document),
        AudioClip {
            template: Some("b_played".to_string()),
            track: "main".to_string(),
            clip: "burst".to_string(),
            index: 0,
        }
    );
}

#[test]
fn a_template_sounds_at_its_clip_placement_offset_and_speed() {
    // Placed at 1 s: the burst plays from 1.5 s for 0.25 s.
    assert_eq!(
        burst_span(json!({ "composition": "burst", "start": 30 }), 30.0, 30.0),
        Some((3 * RATE / 2 + 1, 7 * RATE / 4 - 1))
    );
    // Trimmed by 0.25 s of the template: from 1.25 s.
    assert_eq!(
        burst_span(
            json!({ "composition": "burst", "start": 30, "offset": 7.5 }),
            30.0,
            30.0
        ),
        Some((5 * RATE / 4 + 1, 3 * RATE / 2 - 1))
    );
    // Twice as fast: from 1.25 s for 0.125 s.
    assert_eq!(
        burst_span(
            json!({ "composition": "burst", "start": 30, "speed": 2 }),
            30.0,
            30.0
        ),
        Some((5 * RATE / 4 + 1, 11 * RATE / 8 - 1))
    );
    // A 24 fps template in a 30 fps composition keeps its own seconds.
    assert_eq!(
        burst_span(json!({ "composition": "burst", "start": 30 }), 24.0, 30.0),
        Some((3 * RATE / 2 + 1, 7 * RATE / 4 - 1))
    );
    // A window that ends at 1.6 s cuts the burst there.
    assert_eq!(
        burst_span(
            json!({ "composition": "burst", "start": 30, "duration": 18 }),
            30.0,
            30.0
        ),
        Some((3 * RATE / 2 + 1, 8 * RATE / 5 - 1))
    );
    // Before the window starts nothing plays, and a held clip is silent past its window.
    assert_eq!(
        burst_span(
            json!({ "composition": "burst", "start": 30, "duration": 12, "end": "hold" }),
            30.0,
            30.0
        ),
        None
    );
}

#[test]
fn speed_resamples_the_source_so_pitch_follows() {
    let provider = TestProvider::with(&[("ramp", ramp(2.0))]);
    let document = document(
        30.0,
        90,
        None,
        json!({ "burst": burst_template(30.0, "ramp", json!({})) }),
        vec![clip_node(json!({ "composition": "burst", "speed": 0.8 }))],
    );
    let composition = composition(&document);
    let mixed = mix(&composition, &provider, 0, 3 * RATE, 3 * RATE);
    // The burst starts at 0.5 s of the template, 0.625 s of the root at 0.8x. Away from its edges
    // each root sample reads 0.8 of a source sample further on.
    let start = 30_000;
    for offset in [1_000, 1_001, 5_000, 9_999] {
        let source = offset as f64 * 0.8;
        assert!(
            (f64::from(mixed[start + offset]) - source / RATE as f64).abs() < 1e-6,
            "at {offset}"
        );
    }
}

#[test]
fn looped_and_nested_clips_compose() {
    let provider = TestProvider::with(&[("tone", constant(1.0, 2.0))]);
    let document = document(
        30.0,
        180,
        None,
        json!({
            "burst": burst_template(30.0, "tone", json!({})),
            "outer": template(30.0, 120, None, vec![clip_node(json!({ "composition": "burst", "start": 15 }))])
        }),
        vec![clip_node(
            json!({ "composition": "outer", "start": 30, "duration": 45, "end": "loop" }),
        )],
    );
    let composition = composition(&document);
    let mixed = mix(&composition, &provider, 0, 6 * RATE, 6 * RATE);
    // The burst is 1 s into `outer` and 1 s into the root: 2 s, then every 1.5 s of the loop.
    for start in [2.0, 3.5, 5.0] {
        let first = (start * RATE as f64) as usize;
        assert_eq!(mixed[first - 1], 0.0, "before {start}");
        assert!(mixed[first + 1] > 0.0, "at {start}");
        assert!(
            (mixed[first + RATE / 8] - 1.0).abs() < 1e-6,
            "inside {start}"
        );
        assert!(mixed[first + RATE / 4] < 1e-6, "after {start}");
    }
}

#[test]
fn each_instance_has_its_own_props_and_time() {
    let provider = TestProvider::with(&[("tone", constant(1.0, 2.0))]);
    let mut sound = burst_template(30.0, "tone", json!({ "volume": "=prop('level')" }));
    sound["props"] = json!({ "level": { "type": "number", "default": 1 } });
    let document = document(
        30.0,
        120,
        None,
        json!({ "sound": sound }),
        vec![
            clip_node(json!({ "composition": "sound", "props": { "level": 0.5 } })),
            clip_node(
                json!({ "composition": "sound", "start": 30, "speed": 2, "props": { "level": 0.25 } }),
            ),
        ],
    );
    let composition = composition(&document);
    let mixed = mix(&composition, &provider, 0, 4 * RATE, 4 * RATE);
    assert!((mixed[5 * RATE / 8] - 0.5).abs() < 1e-6);
    assert!((mixed[21 * RATE / 16] - 0.25).abs() < 1e-6);
}

#[test]
fn a_time_remap_moves_audio_and_silences_it_running_backwards() {
    let provider = TestProvider::with(&[("tone", constant(1.0, 2.0))]);
    let remapped = |frame: &str| {
        let (nodes, connections) = graph(vec![]);
        let mut nodes = nodes.as_array().unwrap().clone();
        let mut connections = connections.as_array().unwrap().clone();
        nodes.push(json!({ "id": 5, "type": "clip", "params": { "composition": "burst" } }));
        nodes.push(json!({ "id": 6, "type": "time_remap", "params": { "frame": frame } }));
        nodes.push(json!({ "id": 7, "type": "merge" }));
        connections.retain(|connection| connection["to_node"] != 2);
        connections.extend([
            json!({ "from_node": 5, "to_node": 6, "to_port": "source" }),
            json!({ "from_node": 1, "to_node": 7, "to_port": "base" }),
            json!({ "from_node": 6, "to_node": 7, "to_port": "overlay" }),
            json!({ "from_node": 7, "to_node": 2, "to_port": "source" }),
        ]);
        let document = json!({
            "timeline": { "fps": 30, "duration_frames": 120 },
            "render_settings": { "width": 4, "height": 4 },
            "compositions": { "burst": burst_template(30.0, "tone", json!({})) },
            "nodes": nodes,
            "connections": connections
        });
        let composition = composition(&document);
        audible_span(&mix(&composition, &provider, 0, 4 * RATE, 4 * RATE), 0.0)
    };
    // Shifted back by 1 s: the burst plays from 1.5 s.
    let (first, last) = remapped("=frame - 30").unwrap();
    assert!(first.abs_diff(3 * RATE / 2) <= 1 && last.abs_diff(7 * RATE / 4) <= 1);
    assert_eq!(remapped("=90 - frame"), None);
}

#[test]
fn gains_that_read_what_audio_has_no_value_for_are_rejected() {
    let document = |volume: &str, in_template: bool| {
        let clip = audio_clip("c", "tone", 0, 100, json!({ "volume": volume }));
        if in_template {
            let mut sound = template(30.0, 30, Some(audio(vec![clip])), vec![]);
            sound["props"] = json!({ "level": { "type": "number" } });
            document(30.0, 30, None, json!({ "sound": sound }), vec![])
        } else {
            document(30.0, 30, Some(audio(vec![clip])), json!({}), vec![])
        }
    };
    let rejected = |volume: &str, in_template: bool| {
        let error = crate::json::parse_value(&document(volume, in_template)).unwrap_err();
        let diagnostic = &error.diagnostics()[0];
        assert_eq!(
            diagnostic.code, "audio.invalid",
            "{volume}: {}",
            diagnostic.message
        );
        (diagnostic.message.clone(), diagnostic.path.clone())
    };
    for (volume, read) in [
        ("=node(1, 'opacity')", "reads node 1"),
        ("=glyph / 2", "reads `glyph`"),
        ("=text_width('hi') / 100", "measures text with `text_width`"),
        (
            "=prop('level')",
            "only available inside a composition template",
        ),
    ] {
        let (message, path) = rejected(volume, false);
        assert!(message.contains(read), "{message}");
        assert!(message.contains("clip `c` volume"), "{message}");
        assert_eq!(path, "/audio");
    }
    let (message, path) = rejected("=prop('missing')", true);
    assert!(message.contains("unknown prop `missing`"), "{message}");
    assert_eq!(path, "/compositions/sound/audio");
    composition(&document("=prop('level') * time", true));
}

#[test]
fn clips_nested_past_the_depth_limit_fail_instead_of_recursing() {
    let depth = crate::scope::MAX_GRAPH_DEPTH as usize;
    let mut compositions = serde_json::Map::new();
    compositions.insert(
        "t0".to_string(),
        template(
            30.0,
            30,
            Some(audio(vec![audio_clip("c", "tone", 0, 100, json!({}))])),
            vec![],
        ),
    );
    for level in 1..depth {
        compositions.insert(
            format!("t{level}"),
            template(
                30.0,
                30,
                None,
                vec![clip_node(
                    json!({ "composition": format!("t{}", level - 1) }),
                )],
            ),
        );
    }
    let document = document(
        30.0,
        30,
        None,
        Value::Object(compositions),
        vec![clip_node(
            json!({ "composition": format!("t{}", depth - 1) }),
        )],
    );
    let composition = composition(&document);
    let provider = TestProvider::with(&[("tone", constant(1.0, 1.0))]);
    let error = AudioMixer::new(&composition, &provider)
        .mix_range(0, 16)
        .unwrap_err();
    assert!(error.to_string().contains("nested"), "{error}");
}

#[test]
fn nested_audio_sounds_right_after_a_parent_loop_restarts() {
    let provider = TestProvider::with(&[("tone", constant(1.0, 1.0))]);
    let document = document(
        30.0,
        120,
        None,
        json!({
            "blip": template(
                30.0,
                30,
                Some(audio(vec![audio_clip("c", "tone", 0, 200, json!({}))])),
                vec![],
            ),
            "outer": template(
                30.0,
                30,
                None,
                vec![clip_node(json!({ "composition": "blip", "duration": 1 }))],
            ),
        }),
        vec![clip_node(
            json!({ "composition": "outer", "duration": 30, "end": "loop" }),
        )],
    );
    let composition = composition(&document);
    // The restart is at sample 48000, 384 samples into the interval; the blip lasts 1 frame.
    let mixed = mix(&composition, &provider, 47_616, 512, 512);
    assert!(mixed[..384].iter().all(|sample| *sample == 0.0));
    assert!((f64::from(mixed[384 + 100]) - 100.0 / 384.0).abs() < 1e-6);
    assert!((f64::from(mixed[511]) - 127.0 / 384.0).abs() < 1e-6);
}

#[test]
fn a_loop_restart_reads_the_props_of_the_frame_it_begins_on() {
    let provider = TestProvider::with(&[("tone", constant(1.0, 1.0))]);
    let mut outer = template(
        30.0,
        30,
        None,
        vec![clip_node(
            json!({ "composition": "blip", "duration": 1, "start": "=prop('at')" }),
        )],
    );
    outer["props"] = json!({ "at": { "type": "number", "default": 0 } });
    let document = document(
        30.0,
        120,
        None,
        json!({
            "blip": template(
                30.0,
                30,
                Some(audio(vec![audio_clip("c", "tone", 0, 200, json!({}))])),
                vec![],
            ),
            "outer": outer,
        }),
        vec![clip_node(json!({
            "composition": "outer",
            "duration": 30,
            "end": "loop",
            "props": { "at": "=if(frame >= 30, 0, 2)" },
        }))],
    );
    let composition = composition(&document);
    // The restart at sample 48000 is the first frame whose props start the blip at once.
    let mixed = mix(&composition, &provider, 47_616, 512, 512);
    assert!(mixed[..384].iter().all(|sample| *sample == 0.0));
    assert!((f64::from(mixed[384 + 100]) - 100.0 / 384.0).abs() < 1e-6);
}

#[test]
fn props_set_by_expression_move_the_gain_between_control_points() {
    let provider = TestProvider::with(&[("tone", constant(1.0, 3.0))]);
    let mut inner = template(
        30.0,
        120,
        Some(audio(vec![audio_clip(
            "c",
            "tone",
            0,
            1_500,
            json!({ "volume": "=prop('level')" }),
        )])),
        vec![],
    );
    inner["props"] = json!({ "level": { "type": "number", "default": 0 } });
    let mut outer = template(
        30.0,
        120,
        None,
        vec![clip_node(
            json!({ "composition": "inner", "props": { "level": "=prop('level') * 2" } }),
        )],
    );
    outer["props"] = json!({ "level": { "type": "number", "default": 0 } });
    let document = document(
        30.0,
        120,
        None,
        json!({ "inner": inner, "outer": outer }),
        vec![clip_node(
            json!({ "composition": "outer", "speed": 2, "props": { "level": "=time" } }),
        )],
    );
    let composition = composition(&document);
    let mixed = mix(&composition, &provider, 0, RATE / 2, RATE / 2);
    // Twice the root's time, a value that rises smoothly instead of stepping every interval.
    for sample in [1_000, 12_345, 20_000] {
        let expected = 2.0 * seconds(sample);
        assert!(
            (f64::from(mixed[sample]) - expected).abs() < 1e-6,
            "at {sample}: {} vs {expected}",
            mixed[sample]
        );
    }
}

#[test]
fn a_looping_time_remap_keeps_sounding_across_its_wrap() {
    let provider = TestProvider::with(&[("tone", constant(1.0, 3.0))]);
    let (nodes, connections) = graph(vec![]);
    let mut nodes = nodes.as_array().unwrap().clone();
    let mut connections = connections.as_array().unwrap().clone();
    nodes.push(json!({ "id": 5, "type": "clip", "params": { "composition": "burst" } }));
    nodes.push(json!({
        "id": 6, "type": "time_remap",
        "params": { "frame": "=frame", "loop_enabled": true, "loop_start": 0, "loop_end": 30 }
    }));
    nodes.push(json!({ "id": 7, "type": "merge" }));
    connections.retain(|connection| connection["to_node"] != 2);
    connections.extend([
        json!({ "from_node": 5, "to_node": 6, "to_port": "source" }),
        json!({ "from_node": 1, "to_node": 7, "to_port": "base" }),
        json!({ "from_node": 6, "to_node": 7, "to_port": "overlay" }),
        json!({ "from_node": 7, "to_node": 2, "to_port": "source" }),
    ]);
    let burst = template(
        30.0,
        90,
        Some(audio(vec![audio_clip("c", "tone", 0, 2_000, json!({}))])),
        vec![],
    );
    let document = json!({
        "timeline": { "fps": 30, "duration_frames": 120 },
        "render_settings": { "width": 4, "height": 4 },
        "compositions": { "burst": burst },
        "nodes": nodes,
        "connections": connections
    });
    let composition = composition(&document);
    // The loop restarts at sample 48000, 384 samples into the interval.
    let mixed = mix(&composition, &provider, 47_616, 512, 512);
    assert!(mixed[..384].iter().all(|sample| *sample == 1.0));
    assert!(mixed[384].abs() < 1e-6);
    assert!((f64::from(mixed[384 + 100]) - 100.0 / 384.0).abs() < 1e-6);
    assert!((f64::from(mixed[511]) - 127.0 / 384.0).abs() < 1e-6);
}

/// `levels` templates, each playing the one before it twice, the first of which has sound, and a
/// root that plays the last once.
fn fan_out(levels: usize) -> Value {
    let mut compositions = serde_json::Map::new();
    compositions.insert(
        "t0".to_string(),
        template(
            30.0,
            30,
            Some(audio(vec![audio_clip("c", "tone", 0, 100, json!({}))])),
            vec![],
        ),
    );
    for level in 1..levels {
        let plays = clip_node(json!({ "composition": format!("t{}", level - 1) }));
        compositions.insert(
            format!("t{level}"),
            template(30.0, 30, None, vec![plays.clone(), plays]),
        );
    }
    document(
        30.0,
        30,
        None,
        Value::Object(compositions),
        vec![clip_node(
            json!({ "composition": format!("t{}", levels - 1) }),
        )],
    )
}

#[test]
fn templates_that_play_sound_too_many_times_fail_validation_quickly() {
    for levels in [14, 40] {
        let started = std::time::Instant::now();
        let error = crate::json::parse_value(&fan_out(levels)).unwrap_err();
        let diagnostic = &error.diagnostics()[0];
        assert_eq!(diagnostic.code, "audio.invalid", "{levels} levels");
        assert!(
            diagnostic.message.contains("at most 4096"),
            "{}",
            diagnostic.message
        );
        assert!(started.elapsed().as_secs_f64() < 2.0, "{levels} levels");
    }
    composition(&fan_out(10));
}

#[test]
fn planning_stops_when_too_many_clips_play_at_once() {
    let wide = composition(&fan_out(11));
    let error = plan(&wide, None, 0, 512).unwrap_err();
    assert_eq!(error.code(), "audio.limit");
    assert!(
        error.to_string().contains("clips and time remaps"),
        "{error}"
    );
    let within = composition(&fan_out(10));
    assert_eq!(plan(&within, None, 0, 512).unwrap().voices.len(), 512);
}

#[test]
fn the_preview_plan_has_a_size_limit() {
    let animated = |frames: u32, fps: f64| {
        composition(&document(
            fps,
            frames,
            Some(json!({
                "tracks": [{ "id": "main", "volume": "=0.5 + sin(time) / 2" }],
                "clips": [audio_clip("c", "tone", 0, 1_000, json!({}))]
            })),
            json!({}),
            vec![],
        ))
    };
    let error = plan::preview_plan_within(&animated(300, 30.0), None, 50).unwrap_err();
    assert_eq!(error.code(), "audio.limit");
    assert!(error.to_string().contains("segments"), "{error}");
    let error = preview_plan(&animated(12_000, 1.0), None).unwrap_err();
    assert!(error.to_string().contains("control intervals"), "{error}");
    plan::preview_plan_within(&animated(300, 30.0), None, 5_000).unwrap();
}

/// A root whose output shows `chain`, from the output down, ending in a clip that plays a
/// template with two seconds of sound. A `switch` selects its next node over a background.
fn chained(chain: Vec<Value>) -> Composition {
    let mut nodes = vec![
        json!({ "id": 1, "type": "background" }),
        json!({ "id": 2, "type": "media_output" }),
    ];
    let mut connections = vec![json!({ "from_node": 5, "to_node": 2, "to_port": "source" })];
    let last = chain.len() - 1;
    for (index, mut node) in chain.into_iter().enumerate() {
        let id = 5 + index as u64;
        node["id"] = json!(id);
        if index < last {
            let ports: &[(u64, &str)] = if node["type"] == "switch" {
                &[(id + 1, "layers"), (1, "layers")]
            } else {
                &[(id + 1, "source")]
            };
            for (from, port) in ports {
                connections.push(json!({ "from_node": from, "to_node": id, "to_port": port }));
            }
        }
        nodes.push(node);
    }
    let burst = template(
        30.0,
        90,
        Some(audio(vec![audio_clip("c", "tone", 0, 2_000, json!({}))])),
        vec![],
    );
    composition(&json!({
        "timeline": { "fps": 30, "duration_frames": 120 },
        "render_settings": { "width": 4, "height": 4 },
        "compositions": { "burst": burst },
        "nodes": nodes,
        "connections": connections
    }))
}

fn looping_remap(end: i64) -> Value {
    json!({
        "type": "time_remap",
        "params": { "frame": "=frame", "loop_enabled": true, "loop_start": 0, "loop_end": end }
    })
}

fn burst_clip() -> Value {
    clip_node(json!({ "composition": "burst" }))
}

#[test]
fn a_switch_under_a_looping_time_remap_sounds_after_the_wrap() {
    let switch = json!({
        "type": "switch", "params": { "selected_layer": "=if(frame >= 0, 0, 1)" }
    });
    let bare = chained(vec![looping_remap(30), burst_clip()]);
    let switched = chained(vec![looping_remap(30), switch, burst_clip()]);
    // The loop restarts at sample 48000, 384 samples into the interval.
    let expected = plan(&bare, None, 47_616, 512).unwrap();
    assert!(
        expected
            .segments
            .iter()
            .any(|segment| segment.start == 48_000)
    );
    assert_eq!(plan(&switched, None, 47_616, 512).unwrap(), expected);
}

#[test]
fn a_loop_inside_a_loop_sounds_after_the_outer_wrap() {
    let provider = TestProvider::with(&[("tone", constant(1.0, 3.0))]);
    let switch = json!({
        "type": "switch", "params": { "selected_layer": "=if(frame >= 0, 0, 1)" }
    });
    let composition = chained(vec![
        looping_remap(30),
        looping_remap(10),
        switch,
        burst_clip(),
    ]);
    let mixed = mix(&composition, &provider, 47_616, 512, 512);
    assert!(mixed[..384].iter().all(|sample| *sample == 1.0));
    assert!(mixed[384].abs() < 1e-6);
    assert!((f64::from(mixed[384 + 100]) - 100.0 / 384.0).abs() < 1e-6);
    assert!((f64::from(mixed[511]) - 127.0 / 384.0).abs() < 1e-6);
}

#[test]
fn a_loop_spanning_every_frame_plans_without_overflow() {
    let wide = json!({
        "type": "time_remap",
        "params": {
            "frame": "=frame", "loop_enabled": true,
            "loop_start": i64::MIN, "loop_end": i64::MAX
        }
    });
    let unlooped = chained(vec![
        json!({ "type": "time_remap", "params": { "frame": "=frame" } }),
        burst_clip(),
    ]);
    let expected = plan(&unlooped, None, 47_616, 512).unwrap();
    assert!(!expected.segments.is_empty());
    assert_eq!(
        plan(&chained(vec![wide, burst_clip()]), None, 47_616, 512).unwrap(),
        expected
    );
}

#[test]
fn audio_with_too_many_tracks_or_clips_fails_validation() {
    let tracks: Vec<Value> = (0..=MAX_AUDIO_TRACKS)
        .map(|index| json!({ "id": format!("t{index}") }))
        .collect();
    let error = crate::json::parse_value(&document(
        30.0,
        30,
        Some(json!({ "tracks": tracks, "clips": [] })),
        json!({}),
        vec![],
    ))
    .unwrap_err();
    assert_eq!(error.diagnostics()[0].code, "audio.invalid");
    assert!(error.diagnostics()[0].message.contains("at most 512"));
    let clips: Vec<Value> = (0..=MAX_AUDIO_CLIPS)
        .map(|index| audio_clip(&format!("c{index}"), "tone", 0, 10, json!({})))
        .collect();
    let error = crate::json::parse_value(&document(
        30.0,
        30,
        Some(audio(clips.clone())),
        json!({}),
        vec![],
    ))
    .unwrap_err();
    assert!(error.diagnostics()[0].message.contains("at most 8192"));
    // The same limits hold inside a template.
    let error = crate::json::parse_value(&document(
        30.0,
        30,
        None,
        json!({ "inner": template(30.0, 30, Some(audio(clips)), vec![]) }),
        vec![],
    ))
    .unwrap_err();
    assert!(error.diagnostics()[0].message.contains("at most 8192"));
}

#[test]
fn planning_does_not_look_at_clips_that_are_not_playing() {
    let mut clips: Vec<Value> = (0..MAX_AUDIO_CLIPS - 1)
        .map(|index| {
            let track = format!("t{}", index % MAX_AUDIO_TRACKS);
            audio_clip(
                &format!("late{index}"),
                "tone",
                600_000 + index as u64,
                100,
                json!({ "track_id": track }),
            )
        })
        .collect();
    clips.push(audio_clip(
        "now",
        "tone",
        0,
        100,
        json!({ "track_id": "t0" }),
    ));
    let tracks: Vec<Value> = (0..MAX_AUDIO_TRACKS)
        .map(|index| json!({ "id": format!("t{index}") }))
        .collect();
    let composition = composition(&document(
        30.0,
        30,
        Some(json!({ "tracks": tracks, "clips": clips })),
        json!({}),
        vec![],
    ));
    // Every interval of the first second, which only one clip plays in.
    let intervals = RATE as u64 / CONTROL_FRAMES;
    let examined = plan::clips_examined(&composition, 0, intervals * CONTROL_FRAMES).unwrap();
    assert!(examined <= intervals as usize * 16, "{examined} looked at");
    let planned = plan(&composition, None, 0, 4_800).unwrap();
    assert_eq!(planned.voices.len(), 1);
}

#[test]
fn a_templates_audio_sources_are_listed_with_the_template() {
    let composition = composition(&document(
        30.0,
        30,
        None,
        json!({ "blip": template(30.0, 30, Some(audio(vec![audio_clip("c", "tone.wav", 0, 100, json!({}))])), vec![]) }),
        vec![],
    ));
    let inventory = composition.media_dependencies();
    assert_eq!(inventory.media.len(), 1);
    assert_eq!(inventory.media[0].source, "tone.wav");
    assert_eq!(
        inventory.media[0].uses[0].at,
        crate::media::MediaLocation::AudioClip {
            template: Some("blip".to_string()),
            track: "main".to_string(),
            clip: "c".to_string(),
            index: 0,
        }
    );
}

#[test]
fn a_plan_has_a_size_limit() {
    let clips: Vec<Value> = (0..64)
        .map(|index| audio_clip(&format!("c{index}"), "tone", 0, 1_000, json!({})))
        .collect();
    let composition = composition(&document(
        30.0,
        30,
        Some(json!({
            "tracks": [{ "id": "main", "volume": "=0.5 + sin(time) / 2" }],
            "clips": clips
        })),
        json!({}),
        vec![],
    ));
    let error = plan::plan_within(&composition, None, 0, RATE as u64, 1_000).unwrap_err();
    assert_eq!(error.code(), "audio.limit");
    assert!(error.to_string().contains("segments"), "{error}");
    plan::plan_within(&composition, None, 0, 4 * CONTROL_FRAMES, 1_000).unwrap();
}

/// One repeating stereo pattern per source, so a wrong source offset changes the samples.
#[derive(Serialize)]
struct FixtureSource {
    pattern: Vec<[f32; 2]>,
    frames: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FixturePlan {
    #[serde(flatten)]
    plan: AudioPlan,
    duration_frames: u32,
    fps: f32,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MixFixture {
    name: &'static str,
    /// The plan the browser preview is given for the whole composition.
    timeline: FixturePlan,
    sources: HashMap<String, FixtureSource>,
    start_sample: u64,
    frames: usize,
    expected: Vec<f32>,
}

fn mix_fixture(
    name: &'static str,
    document: Value,
    sources: Vec<(&str, Vec<[f32; 2]>, usize)>,
    start_sample: u64,
    frames: usize,
) -> MixFixture {
    let composition = composition(&document);
    let provider = TestProvider::default();
    for (id, pattern, frames) in &sources {
        let channel = |index: usize| {
            (0..*frames)
                .map(|frame| pattern[frame % pattern.len()][index])
                .collect::<Vec<_>>()
        };
        provider.set(
            id,
            id,
            AudioBuffer::from_channels(vec![channel(0), channel(1)]),
        );
    }
    let expected = AudioMixer::new(&composition, &provider)
        .mix_range(start_sample, frames)
        .unwrap()
        .interleaved_f32();
    MixFixture {
        name,
        timeline: FixturePlan {
            plan: preview_plan(&composition, None).unwrap(),
            duration_frames: composition.timeline().duration_frames,
            fps: composition.timeline().fps,
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
    let root = |tracks: Value, clips: Vec<Value>| {
        document(
            1.0,
            60,
            Some(json!({ "tracks": tracks, "clips": clips })),
            json!({}),
            vec![],
        )
    };
    let main = || json!([{ "id": "main" }]);
    let clip = |id: &str, start_ms: u64, duration_ms: u64| {
        audio_clip(id, "tone", start_ms, duration_ms, json!({}))
    };
    let on = |track: &str, mut clip: Value| {
        clip["track_id"] = json!(track);
        clip
    };
    let long_clip = || clip("clip", 100, 1_000);
    let overlap = |id: &str, source: &str| audio_clip(id, source, 0, 1_000, json!({}));
    let overlap_sources = || {
        vec![
            ("up", vec![[0.8, -0.8]], 96_000),
            ("down", vec![[-0.8, 0.8]], 96_000),
        ]
    };
    let templated = |clip: Value| {
        document(
            30.0,
            90,
            None,
            json!({
                "burst": burst_template(30.0, "tone", json!({ "volume": "=0.5 + prop('lift')", "fade_in_ms": 20 })),
            })
            .as_object()
            .map(|templates| {
                let mut templates = templates.clone();
                templates["burst"]["props"] = json!({ "lift": { "type": "number", "default": 0 } });
                Value::Object(templates)
            })
            .unwrap(),
            vec![clip_node(clip)],
        )
    };

    vec![
        mix_fixture(
            "start position and source offset",
            root(
                main(),
                vec![audio_clip(
                    "clip",
                    "tone",
                    100,
                    1_000,
                    json!({ "source_start_ms": 2 }),
                )],
            ),
            tone(),
            6_000,
            16,
        ),
        mix_fixture(
            "track and clip gain multiply",
            root(
                json!([{ "id": "main", "volume": 0.5 }]),
                vec![audio_clip(
                    "clip",
                    "tone",
                    100,
                    1_000,
                    json!({ "volume": 0.25 }),
                )],
            ),
            tone(),
            6_000,
            16,
        ),
        mix_fixture(
            "fades in over 384 samples from the clip start",
            root(main(), vec![long_clip()]),
            tone(),
            4_796,
            16,
        ),
        mix_fixture(
            "fade-in reaches full gain",
            root(main(), vec![long_clip()]),
            tone(),
            5_176,
            16,
        ),
        mix_fixture(
            "fades out over the last 384 samples",
            root(main(), vec![long_clip()]),
            tone(),
            52_416,
            16,
        ),
        mix_fixture(
            "reaches silence at the clip end",
            root(main(), vec![long_clip()]),
            tone(),
            52_792,
            16,
        ),
        mix_fixture(
            "a short clip fades over half its length",
            root(main(), vec![clip("clip", 0, 10)]),
            tone(),
            232,
            16,
        ),
        mix_fixture(
            "a very short clip fades over half its length",
            root(main(), vec![clip("clip", 0, 1)]),
            tone(),
            16,
            40,
        ),
        mix_fixture(
            "a solo track silences the others",
            root(
                json!([{ "id": "solo", "solo": true }, { "id": "other" }]),
                vec![
                    on("solo", clip("kept", 0, 1_000)),
                    on("other", clip("dropped", 0, 1_000)),
                ],
            ),
            tone(),
            1_000,
            16,
        ),
        mix_fixture(
            "a muted track is silent",
            root(
                json!([{ "id": "muted", "muted": true }, { "id": "audible" }]),
                vec![
                    on("muted", clip("dropped", 0, 1_000)),
                    on("audible", clip("kept", 0, 1_000)),
                ],
            ),
            tone(),
            1_000,
            16,
        ),
        mix_fixture(
            "a silent track adds nothing",
            root(json!([{ "id": "main", "volume": 0 }]), vec![long_clip()]),
            tone(),
            6_000,
            16,
        ),
        mix_fixture(
            "overlaps sum before clamping once",
            root(
                main(),
                vec![overlap("a", "up"), overlap("b", "up"), overlap("c", "down")],
            ),
            overlap_sources(),
            1_000,
            4,
        ),
        mix_fixture(
            "the mix clamps in both directions",
            root(main(), vec![overlap("a", "up"), overlap("b", "up")]),
            overlap_sources(),
            1_000,
            4,
        ),
        mix_fixture(
            "a source shorter than its clip ends in silence",
            root(main(), vec![clip("clip", 0, 1_000)]),
            vec![("tone", pattern.clone(), 1_000)],
            992,
            16,
        ),
        mix_fixture(
            "a window can span the gap between two clips",
            root(main(), vec![clip("first", 0, 10), clip("second", 11, 10)]),
            tone(),
            470,
            80,
        ),
        mix_fixture(
            "an animated gain moves linearly between control points",
            root(
                json!([{ "id": "main", "volume": "=0.5 + sin(time * 20) / 2" }]),
                vec![clip("clip", 0, 2_000)],
            ),
            tone(),
            1_000,
            1_200,
        ),
        mix_fixture(
            "equal-power fades in and out",
            root(
                main(),
                vec![audio_clip(
                    "clip",
                    "tone",
                    0,
                    100,
                    json!({ "fade_in_ms": 60, "fade_out_ms": 60 }),
                )],
            ),
            tone(),
            0,
            4_800,
        ),
        mix_fixture(
            "a template plays at its clip's placement",
            templated(json!({ "composition": "burst", "start": 10 })),
            tone(),
            39_000,
            2_000,
        ),
        mix_fixture(
            "a faster template resamples between source samples",
            templated(json!({ "composition": "burst", "speed": 1.5, "props": { "lift": 0.25 } })),
            tone(),
            15_500,
            1_000,
        ),
        mix_fixture(
            "a looped window cuts and restarts its template",
            templated(
                json!({ "composition": "burst", "offset": 10, "duration": 10, "end": "loop" }),
            ),
            tone(),
            15_600,
            1_000,
        ),
    ]
}

/// `packages/lumen-preview/test/audio-worker.test.ts` plays the same plans in the browser mixer, so
/// a change on either side fails until both agree.
#[test]
fn browser_mixer_fixtures_match_this_mixer() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/audio_mix.json");
    let generated = serde_json::to_string_pretty(&mix_fixtures()).unwrap();
    if std::env::var_os("LUMEN_BLESS_AUDIO_FIXTURES").is_some() {
        std::fs::write(path, format!("{generated}\n")).unwrap();
    }
    // Parsed so the committed file's formatting doesn't matter.
    let parse = |text: &str| serde_json::from_str::<Value>(text).unwrap();
    let committed = parse(&std::fs::read_to_string(path).unwrap());
    let generated = parse(&generated);
    assert_eq!(
        generated, committed,
        "the mixer changed: rerun with LUMEN_BLESS_AUDIO_FIXTURES=1, then update the browser mixer to match"
    );
}
