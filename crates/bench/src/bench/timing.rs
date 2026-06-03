use std::time::{Duration, Instant};

/// Accumulates named phase durations for machine-readable bench output.
#[derive(Debug, Default)]
pub struct PhaseTimer {
    phases: Vec<(&'static str, Duration)>,
}

impl PhaseTimer {
    pub fn time<T>(&mut self, name: &'static str, f: impl FnOnce() -> T) -> T {
        let started = Instant::now();
        let value = f();
        self.phases.push((name, started.elapsed()));
        value
    }

    pub async fn time_async<T>(
        &mut self,
        name: &'static str,
        f: impl std::future::Future<Output = T>,
    ) -> T {
        let started = Instant::now();
        let value = f.await;
        self.phases.push((name, started.elapsed()));
        value
    }

    pub fn push(&mut self, name: &'static str, duration: Duration) {
        self.phases.push((name, duration));
    }

    pub fn phases(&self) -> &[(&'static str, Duration)] {
        &self.phases
    }

    pub fn print(&self, prefix: &str) {
        for (name, duration) in &self.phases {
            println!(
                "{prefix} phase={name} ms={} us={:.2}",
                duration.as_millis(),
                duration.as_secs_f64() * 1_000_000.0
            );
        }
    }
}

pub fn micros_per_frame(duration: Duration, frames: u32) -> f64 {
    if frames == 0 {
        return 0.0;
    }
    duration.as_secs_f64() * 1_000_000.0 / f64::from(frames)
}

pub fn fps(frames: u32, elapsed: Duration) -> f64 {
    f64::from(frames) / elapsed.as_secs_f64().max(1e-9)
}
