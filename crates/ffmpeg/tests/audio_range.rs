//! `AudioRangeReader` must return the samples at the positions it is asked for, whatever order the
//! ranges come in, and keep audio that starts late in place.

use std::{
    fs,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use lumen_ffmpeg::AudioRangeReader;

const SAMPLE_RATE: usize = 48_000;
const SAMPLES: usize = 3 * SAMPLE_RATE;

fn temp_path(name: &str, extension: &str) -> std::path::PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!("lumen_audio_range_{name}_{unique}.{extension}"))
}

/// The left channel at `index`; the right channel is its negative.
fn ramp(index: usize) -> f32 {
    (index % 4096) as f32 / 4096.0
}

/// Three seconds of 48 kHz float stereo in which every sample's value tells its position.
fn ramp_wav() -> Vec<u8> {
    let data_len = SAMPLES * 8;
    let mut wav = Vec::with_capacity(44 + data_len);
    wav.extend(b"RIFF");
    wav.extend((36 + data_len as u32).to_le_bytes());
    wav.extend(b"WAVEfmt ");
    wav.extend(16_u32.to_le_bytes());
    wav.extend(3_u16.to_le_bytes());
    wav.extend(2_u16.to_le_bytes());
    wav.extend((SAMPLE_RATE as u32).to_le_bytes());
    wav.extend((SAMPLE_RATE as u32 * 8).to_le_bytes());
    wav.extend(8_u16.to_le_bytes());
    wav.extend(32_u16.to_le_bytes());
    wav.extend(b"data");
    wav.extend((data_len as u32).to_le_bytes());
    for index in 0..SAMPLES {
        wav.extend(ramp(index).to_le_bytes());
        wav.extend((-ramp(index)).to_le_bytes());
    }
    wav
}

fn assert_range(reader: &mut AudioRangeReader, start: usize, frames: usize, expected: usize) {
    let range = reader.read(start as u64, frames).expect("read range");
    assert_eq!(range.samples, expected, "length of {frames} from {start}");
    for index in 0..expected {
        assert_eq!(
            range.interleaved_f32[index * 2],
            ramp(start + index),
            "left sample {} of the range from {start}",
            index
        );
        assert_eq!(
            range.interleaved_f32[index * 2 + 1],
            -ramp(start + index),
            "right sample {} of the range from {start}",
            index
        );
    }
}

#[test]
fn reads_each_range_from_where_it_was_asked() {
    let path = temp_path("ramp", "wav");
    fs::write(&path, ramp_wav()).expect("write wav");
    let mut reader = AudioRangeReader::open(path.to_string_lossy().to_string()).expect("open");

    assert_range(&mut reader, 1_000, 500, 500);
    assert_range(&mut reader, 1_500, 500, 500);
    // Backwards to near the start, then far ahead, then past the end.
    assert_range(&mut reader, 100, 50, 50);
    assert_range(&mut reader, 100_000, 300, 300);
    assert_range(&mut reader, SAMPLES - 200, 500, 200);
    assert_range(&mut reader, SAMPLES + 10_000, 10, 0);
    assert_range(&mut reader, 70_000, 20_000, 20_000);

    let _ = fs::remove_file(path);
}

#[test]
fn audio_that_starts_late_reads_as_silence_until_it_begins() {
    let path = temp_path("late", "mkv");
    let generated = Command::new("ffmpeg")
        .args(["-y", "-hide_banner", "-loglevel", "error"])
        .args(["-f", "lavfi", "-i", "color=c=black:s=16x16:r=10:d=2"])
        .args(["-itsoffset", "0.5"])
        .args([
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=48000:duration=1",
        ])
        .args(["-c:v", "mpeg4", "-c:a", "pcm_s16le"])
        .arg(&path)
        .status();
    if !matches!(generated, Ok(status) if status.success()) {
        eprintln!("ffmpeg CLI unavailable; skipping late audio test");
        return;
    }
    let mut reader = AudioRangeReader::open(path.to_string_lossy().to_string()).expect("open");
    let range = reader.read(0, SAMPLE_RATE).expect("read range");
    let _ = fs::remove_file(path);

    assert_eq!(range.samples, SAMPLE_RATE);
    let peak = |from: usize, to: usize| {
        range.interleaved_f32[from * 2..to * 2]
            .iter()
            .fold(0.0_f32, |peak, sample| peak.max(sample.abs()))
    };
    assert_eq!(peak(0, 23_000), 0.0, "before the audio begins");
    assert!(peak(25_000, SAMPLE_RATE) > 0.05, "after the audio begins");
}
