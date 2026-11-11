#![cfg(target_os = "linux")]

//! Renders template audio through the real `lumen-local` binary on lavapipe and checks the
//! envelope of the encoded sound. Skips like the engine's GPU tests when lavapipe isn't installed.

use std::{
    f64::consts::PI,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use lumen_ffmpeg::AudioRangeReader;
use serde_json::json;

const RATE: usize = 48_000;
const TONE_HZ: f64 = 440.0;
const AMPLITUDE: f64 = 0.5;

fn lavapipe_icd() -> Option<PathBuf> {
    let icd = fs::read_dir("/usr/share/vulkan/icd.d")
        .ok()?
        .filter_map(|entry| Some(entry.ok()?.path()))
        .find(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("lvp_icd"))
        });
    if icd.is_none() {
        eprintln!("lavapipe not installed; skipping lumen-local audio test");
    }
    icd
}

/// One second of a stereo 16-bit sine, the same in both channels.
fn write_tone(path: &Path) {
    let samples: Vec<i16> = (0..RATE)
        .map(|n| {
            let value = AMPLITUDE * (2.0 * PI * TONE_HZ * n as f64 / RATE as f64).sin();
            (value * f64::from(i16::MAX)).round() as i16
        })
        .collect();
    let data_len = (samples.len() * 4) as u32;
    let mut wav = Vec::new();
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_len).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16_u32.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&2_u16.to_le_bytes());
    wav.extend_from_slice(&(RATE as u32).to_le_bytes());
    wav.extend_from_slice(&(RATE as u32 * 4).to_le_bytes());
    wav.extend_from_slice(&4_u16.to_le_bytes());
    wav.extend_from_slice(&16_u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    for sample in samples {
        wav.extend_from_slice(&sample.to_le_bytes());
        wav.extend_from_slice(&sample.to_le_bytes());
    }
    fs::write(path, wav).expect("write tone");
}

/// A 0.5 s template sound with a 100 ms fade in, played at 0.5 s and again at 1.5 s twice as fast.
fn composition() -> serde_json::Value {
    let background = |id: u64| json!({ "id": id, "type": "background", "params": { "width": 64, "height": 64 } });
    json!({
        "timeline": { "fps": 30, "duration_frames": 75 },
        "render_settings": { "width": 64, "height": 64 },
        "compositions": {
            "sting": {
                "timeline": { "fps": 30, "duration_frames": 30 },
                "audio": {
                    "tracks": [{ "id": "main" }],
                    "clips": [{
                        "id": "tone", "source_id": "tone.wav", "track_id": "main",
                        "start_ms": 0, "duration_ms": 500, "fade_in_ms": 100
                    }]
                },
                "nodes": [background(1), { "id": 2, "type": "media_output" }],
                "connections": [{ "from_node": 1, "to_node": 2, "to_port": "source" }]
            }
        },
        "nodes": [
            background(1),
            { "id": 3, "type": "clip", "params": { "composition": "sting", "start": 15 } },
            { "id": 4, "type": "clip", "params": { "composition": "sting", "start": 45, "speed": 2 } },
            { "id": 5, "type": "merge" },
            { "id": 6, "type": "merge" },
            { "id": 2, "type": "media_output" }
        ],
        "connections": [
            { "from_node": 1, "to_node": 5, "to_port": "base" },
            { "from_node": 3, "to_node": 5, "to_port": "overlay" },
            { "from_node": 5, "to_node": 6, "to_port": "base" },
            { "from_node": 4, "to_node": 6, "to_port": "overlay" },
            { "from_node": 6, "to_node": 2, "to_port": "source" }
        ]
    })
}

/// The left channel's RMS over `[from, to)` seconds.
fn rms(left: &[f32], from: f64, to: f64) -> f64 {
    let window = &left[(from * RATE as f64) as usize..(to * RATE as f64) as usize];
    (window.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / window.len() as f64).sqrt()
}

/// Upward zero crossings per second over `[from, to)` seconds: the frequency of a sine.
fn frequency(left: &[f32], from: f64, to: f64) -> f64 {
    let window = &left[(from * RATE as f64) as usize..(to * RATE as f64) as usize];
    let crossings = window
        .windows(2)
        .filter(|pair| pair[0] < 0.0 && pair[1] >= 0.0)
        .count();
    crossings as f64 / (to - from)
}

#[test]
fn template_audio_plays_at_each_clip_placement_and_speed() {
    let Some(icd) = lavapipe_icd() else { return };
    let directory = tempfile::tempdir().expect("temp dir");
    write_tone(&directory.path().join("tone.wav"));
    let composition_path = directory.path().join("composition.json");
    fs::write(&composition_path, composition().to_string()).expect("write composition");
    let video = directory.path().join("out.mp4");

    let output = Command::new(env!("CARGO_BIN_EXE_lumen-local"))
        .args(["--composition", composition_path.to_str().unwrap()])
        .args(["--media-root", directory.path().to_str().unwrap()])
        .args(["--output", video.to_str().unwrap()])
        .args(["--encoder", "libx264"])
        .env_remove("LUMEN_VIDEO_ENCODER")
        .env("VK_ICD_FILENAMES", &icd)
        .env("LUMEN_GPU_FORCE_FALLBACK_ADAPTER", "1")
        .env("WGPU_ALLOW_UNDERLYING_NONCOMPLIANT_ADAPTER", "1")
        .env("WGPU_BACKEND", "vulkan")
        .env("WGPU_POWER_PREF", "none")
        .env("NO_COLOR", "1")
        .output()
        .expect("run lumen-local");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let mut reader = AudioRangeReader::open(video.to_str().unwrap()).expect("open audio");
    let decoded = reader.read(0, RATE * 5 / 2).expect("read audio");
    let left: Vec<f32> = decoded.interleaved_f32.iter().step_by(2).copied().collect();
    let full = AMPLITUDE / 2_f64.sqrt();

    for (from, to) in [(0.0, 0.45), (1.05, 1.45), (1.8, 2.4)] {
        assert!(
            rms(&left, from, to) < 0.01,
            "silent from {from} s to {to} s"
        );
    }
    // The first instance: the equal-power fade is at sin(π/4) of full halfway through.
    let halfway = rms(&left, 0.545, 0.555);
    assert!(
        (halfway - full * (PI / 4.0).sin()).abs() < 0.04,
        "fade halfway: {halfway}"
    );
    let steady = rms(&left, 0.65, 0.95);
    assert!((steady - full).abs() < 0.02, "first instance: {steady}");
    assert!((frequency(&left, 0.65, 0.95) - TONE_HZ).abs() < 10.0);
    // The second plays the same 0.5 s in 0.25 s, an octave up.
    let fast = rms(&left, 1.58, 1.72);
    assert!((fast - full).abs() < 0.02, "second instance: {fast}");
    assert!((frequency(&left, 1.58, 1.72) - 2.0 * TONE_HZ).abs() < 20.0);
}
