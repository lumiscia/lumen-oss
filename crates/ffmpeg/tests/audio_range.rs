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
            "-af",
            "asetpts=PTS+if(gte(T\\,1)\\,1/TB\\,0)",
            "-c:a",
            "pcm_s16le",
        ],
        &path,
    ) {
        return;
    }
    let mut reader = AudioRangeReader::open(path.to_string_lossy().to_string()).expect("open");
    let range = reader.read(0, 5 * SAMPLE_RATE).expect("read range");
    let _ = fs::remove_file(path);

    // Matroska timestamps are whole milliseconds, so the end can be up to a millisecond off.
    assert!(
        range.samples.abs_diff(4 * SAMPLE_RATE) < 48,
        "{} samples",
        range.samples
    );
    assert!(peak(&range, 10_000, 40_000) > 0.05, "before the gap");
    assert_eq!(peak(&range, 52_800, 91_200), 0.0, "in the gap");
    assert!(peak(&range, 100_000, 180_000) > 0.05, "after the gap");
}

const CHIRP: &str = "aevalsrc=0.4*sin(2*PI*(300+200*t)*t):d=6:s=48000";

#[test]
fn frames_wholly_behind_the_clock_are_dropped() {
    let path = temp_path("restart", "mkv");
    if !generate(
        &[
            "-f",
            "lavfi",
            "-i",
            CHIRP,
            "-af",
            "asetpts=PTS-if(gte(T\\,2)\\,1/TB\\,0)",
            "-c:a",
            "pcm_s16le",
        ],
        &path,
    ) {
        return;
    }
    let path_string = path.to_string_lossy().to_string();
    let continuous = AudioRangeReader::open(path_string.clone())
        .expect("open")
        .read(0, 6 * SAMPLE_RATE)
        .expect("read all");
    let seeked = AudioRangeReader::open(path_string)
        .expect("open")
        .read(4 * SAMPLE_RATE as u64, SAMPLE_RATE)
        .expect("read seeked");
    let _ = fs::remove_file(path);

    // The second after the timestamps step back repeats audio that was already placed.
    assert!(
        continuous.samples.abs_diff(5 * SAMPLE_RATE) < 48,
        "{} samples",
        continuous.samples
    );
    // A seek lands within a millisecond of the continuous read.
    let difference = |lag: usize| {
        (0..seeked.samples * 2)
            .map(|i| {
                (seeked.interleaved_f32[i]
                    - continuous.interleaved_f32[4 * SAMPLE_RATE * 2 - 96 + lag + i])
                    .abs()
            })
            .fold(0.0_f32, f32::max)
    };
    assert!(
        (0..=192).any(|lag| difference(lag) < 1e-3),
        "seeked read is not the continuous audio"
    );
}

#[test]
fn a_gap_before_a_rate_change_is_kept_in_mpegts() {
    let (first, second, joined) = (
        temp_path("first", "ts"),
        temp_path("second", "ts"),
        temp_path("joined", "ts"),
    );
    let source = |rate: u32| format!("aevalsrc=0.4*sin(2*PI*(300+200*t)*t):d=3:s={rate}");
    if !generate(
        &[
            "-f",
            "lavfi",
            "-i",
            &source(48_000),
            "-c:a",
            "aac",
            "-muxdelay",
            "0",
            "-f",
            "mpegts",
        ],
        &first,
    ) || !generate(
        &[
            "-f",
            "lavfi",
            "-i",
            &source(32_000),
            "-c:a",
            "aac",
            "-muxdelay",
            "0",
            "-output_ts_offset",
            "5",
            "-f",
            "mpegts",
        ],
        &second,
    ) {
        return;
    }
    let mut bytes = fs::read(&first).expect("read first");
    bytes.extend(fs::read(&second).expect("read second"));
    fs::write(&joined, bytes).expect("write joined");

    let mut reader = AudioRangeReader::open(joined.to_string_lossy().to_string()).expect("open");
    let range = reader.read(0, 9 * SAMPLE_RATE).expect("read range");
    for path in [first, second, joined] {
        let _ = fs::remove_file(path);
    }

    assert!(
        range.samples.abs_diff(8 * SAMPLE_RATE) < 4_096,
        "{} samples",
        range.samples
    );
    assert!(peak(&range, 10_000, 130_000) > 0.05, "before the gap");
    assert_eq!(
        peak(&range, 3 * SAMPLE_RATE + 9_600, 5 * SAMPLE_RATE - 4_800),
        0.0,
        "in the gap"
    );
    assert!(
        peak(&range, 5 * SAMPLE_RATE + 9_600, 7 * SAMPLE_RATE) > 0.05,
        "after the gap"
    );
}
