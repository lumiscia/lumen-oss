use std::time::Duration;

pub fn micros_per_frame(duration: Duration, frames: u32) -> f64 {
    if frames == 0 {
        return 0.0;
    }
    duration.as_secs_f64() * 1_000_000.0 / f64::from(frames)
}

pub fn fps(frames: u32, elapsed: Duration) -> f64 {
    f64::from(frames) / elapsed.as_secs_f64().max(1e-9)
}
