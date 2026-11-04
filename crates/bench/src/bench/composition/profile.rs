use std::time::{Duration, Instant};

use lumen_engine::gpu::GpuCompositionRenderer;

use crate::bench::timing::micros_per_frame;

/// Time spent in each named phase of a `--profile` run.
#[derive(Debug, Default)]
pub struct PhaseTimes {
    phases: Vec<(&'static str, Duration)>,
}

impl PhaseTimes {
    fn add(&mut self, name: &'static str, elapsed: Duration) {
        match self.phases.iter_mut().find(|(phase, _)| *phase == name) {
            Some((_, total)) => *total += elapsed,
            None => self.phases.push((name, elapsed)),
        }
    }

    pub fn print(&self, mode: &str, frames: u32) {
        let mut line = format!("composition_profile mode={mode} frames={frames}");
        for (name, total) in &self.phases {
            line.push_str(&format!(
                " {name}_ms={} {name}_us_per_frame={:.2}",
                total.as_millis(),
                micros_per_frame(*total, frames)
            ));
        }
        println!("{line}");
    }
}

/// Runs `f`, adding its duration to `name` when profiling. Without a profile no clock is read.
pub fn timed<T>(
    phases: &mut Option<&mut PhaseTimes>,
    name: &'static str,
    f: impl FnOnce() -> T,
) -> T {
    let Some(phases) = phases.as_deref_mut() else {
        return f();
    };
    let started = Instant::now();
    let value = f();
    phases.add(name, started.elapsed());
    value
}

/// Prints what the renderer has recorded so far and the GPU memory it holds.
pub fn print_render_profile(renderer: &GpuCompositionRenderer) {
    let stats = renderer.frame_graph_stats();
    println!(
        "render_profile passes={} programs={} bind_groups={} textures_created={} held_bytes={} held_mib={:.1}",
        stats.recorder.passes,
        stats.programs,
        stats.bind_groups,
        stats.recorder.textures_created,
        stats.held_bytes,
        stats.held_bytes as f64 / (1024.0 * 1024.0)
    );
}
