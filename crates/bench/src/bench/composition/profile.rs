use std::time::{Duration, Instant};

use lumen_engine::gpu::CompiledComposition;
use lumen_gpu::PassDesc;

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

pub fn print_plan_profile(compiled: Option<&CompiledComposition>) {
    let Some(compiled) = compiled else {
        return;
    };
    println!(
        "plan_profile textures={} buffers={} programs={} passes={} compiled_nodes={}",
        compiled.plan.textures().len(),
        compiled.plan.buffers().len(),
        compiled.plan.programs().len(),
        compiled.plan.passes().len(),
        compiled.compiled_nodes.len(),
    );
    for pass in compiled.plan.passes() {
        let (kind, label) = match &pass.desc {
            PassDesc::Render(desc) => ("render", desc.label.as_deref()),
            PassDesc::Compute(desc) => ("compute", desc.label.as_deref()),
            PassDesc::CopyTexture(desc) => ("copy", desc.label.as_deref()),
        };
        println!(
            "plan_pass id={} kind={} label={}",
            pass.id.0,
            kind,
            label.unwrap_or("-")
        );
    }
}

/// Prints the GPU memory the compiled plan's textures take. The shared text atlas is a
/// placeholder in the plan, so it is left out.
pub fn print_plan_memory(compiled: Option<&CompiledComposition>) {
    let Some(compiled) = compiled else {
        return;
    };
    let textures: Vec<u64> = compiled
        .plan
        .textures()
        .iter()
        .map(|texture| {
            texture
                .desc
                .format
                .theoretical_memory_footprint(texture.desc.domain.storage_size.as_extent())
        })
        .collect();
    println!(
        "plan_memory textures={} plan_texture_bytes={} plan_texture_mib={:.1}",
        textures.len(),
        textures.iter().sum::<u64>(),
        textures.iter().sum::<u64>() as f64 / (1024.0 * 1024.0)
    );
}
