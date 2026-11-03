//! `AudioRangeReader` must return the samples at the positions it is asked for, whatever order the
//! ranges come in, and keep audio that starts late or resumes after a gap in place.

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

/// Three seconds of float stereo in which every sample's value tells its position.
fn ramp_wav(sample_rate: usize) -> Vec<u8> {
    let samples = 3 * sample_rate;
    let data_len = samples * 8;
    let mut wav = Vec::with_capacity(44 + data_len);
    wav.extend(b"RIFF");
    wav.extend((36 + data_len as u32).to_le_bytes());
    wav.extend(b"WAVEfmt ");
    wav.extend(16_u32.to_le_bytes());
    wav.extend(3_u16.to_le_bytes());
    wav.extend(2_u16.to_le_bytes());
    wav.extend((sample_rate as u32).to_le_bytes());
    wav.extend((sample_rate as u32 * 8).to_le_bytes());
    wav.extend(8_u16.to_le_bytes());
    wav.extend(32_u16.to_le_bytes());
    wav.extend(b"data");
    wav.extend((data_len as u32).to_le_bytes());
    for index in 0..samples {
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
    fs::write(&path, ramp_wav(SAMPLE_RATE)).expect("write wav");
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

fn generate(args: &[&str], path: &std::path::Path) -> bool {
    let status = Command::new("ffmpeg")
        .args(["-y", "-hide_banner", "-loglevel", "error"])
        .args(args)
        .arg(path)
        .status();
    let generated = matches!(status, Ok(status) if status.success());
    if !generated {
        eprintln!("ffmpeg CLI unavailable; skipping");
    }
    generated
}

fn peak(range: &lumen_ffmpeg::AudioFrame, from: usize, to: usize) -> f32 {
    range.interleaved_f32[from * 2..to * 2]
        .iter()
        .fold(0.0_f32, |peak, sample| peak.max(sample.abs()))
}

#[test]
fn a_range_is_the_same_whether_reached_by_rewinding_or_decoding_on() {
    let path = temp_path("resampled", "wav");
    fs::write(&path, ramp_wav(11_025)).expect("write wav");
    let path_string = path.to_string_lossy().to_string();

    let continuous = AudioRangeReader::open(path_string.clone())
        .expect("open")
        .read(0, 3 * SAMPLE_RATE)
        .expect("read all");
    let mut reader = AudioRangeReader::open(path_string).expect("open");
    reader.read(60_000, 5_000).expect("read ahead");
    for (start, frames) in [(48, 48_000), (1, 1_000), (90_001, 20_000)] {
        let range = reader.read(start as u64, frames).expect("read range");
        assert_eq!(
            range.interleaved_f32,
            continuous.interleaved_f32[start * 2..(start + frames) * 2],
            "{frames} samples from {start}"
        );
    }
    let _ = fs::remove_file(path);
}

#[test]
fn a_stream_whose_layout_changes_keeps_decoding() {
    let stereo = temp_path("stereo", "aac");
    let mono = temp_path("mono", "aac");
    let joined = temp_path("joined", "aac");
    let sine = |frequency: u32, layout: &str| {
        format!(
            "sine=frequency={frequency}:sample_rate=48000:duration=1,aformat=channel_layouts={layout}"
        )
    };
    if !generate(&["-f", "lavfi", "-i", &sine(440, "stereo")], &stereo)
        || !generate(&["-f", "lavfi", "-i", &sine(880, "mono")], &mono)
    {
        return;
    }
    let mut bytes = fs::read(&stereo).expect("read stereo");
    bytes.extend(fs::read(&mono).expect("read mono"));
    fs::write(&joined, bytes).expect("write joined");

    let mut reader = AudioRangeReader::open(joined.to_string_lossy().to_string()).expect("open");
    let range = reader.read(0, 2 * SAMPLE_RATE).expect("read range");
    for path in [stereo, mono, joined] {
        let _ = fs::remove_file(path);
    }

    assert!(
        range.samples > 2 * SAMPLE_RATE - 4_096,
        "{} samples",
        range.samples
    );
    assert!(peak(&range, 10_000, 40_000) > 0.05, "stereo part");
    assert!(peak(&range, 60_000, 90_000) > 0.05, "mono part");
}

#[test]
fn a_gap_in_the_timestamps_reads_as_silence() {
    let path = temp_path("gap", "mkv");
    if !generate(
        &[
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=48000:duration=3",
        ]
        .iter()
        .chain(&[
            "-af",
            "aselect=not(between(t\\,1\\,2))",
            "-c:a",
            "pcm_s16le",
        ])
        .copied()
        .collect::<Vec<_>>(),
        &path,
    ) {
        return;
    }
    let mut reader = AudioRangeReader::open(path.to_string_lossy().to_string()).expect("open");
    let range = reader.read(0, 3 * SAMPLE_RATE).expect("read range");
    let _ = fs::remove_file(path);

    // Matroska timestamps are whole milliseconds, so the end can be up to a millisecond off.
    assert!(
        range.samples.abs_diff(3 * SAMPLE_RATE) < 48,
        "{} samples",
        range.samples
    );
    assert!(peak(&range, 10_000, 40_000) > 0.05, "before the gap");
    assert_eq!(peak(&range, 50_000, 94_000), 0.0, "in the gap");
    assert!(peak(&range, 100_000, 140_000) > 0.05, "after the gap");
}
