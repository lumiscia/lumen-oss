//! Benchmark harness shared by `lumen-bench-*` binaries.

pub mod composition;
pub mod decode;
pub mod fixtures;
pub mod json_parse;
pub mod media;
pub mod report;
pub mod text;
pub mod timing;

/// A benchmark entry point invoked by a thin binary wrapper.
pub trait Bench {
    fn name() -> &'static str;
    fn run() -> anyhow::Result<()>;
}

/// A reproducible workload built in-process (not loaded from demo JSON).
pub trait CompositionFixture {
    fn name(&self) -> &'static str;
    fn build(&self) -> lumen_engine::composition::Composition;
    fn default_frames(&self, composition: &lumen_engine::composition::Composition) -> u32 {
        composition.timeline.duration_frames.min(90)
    }
}
