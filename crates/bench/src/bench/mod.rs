//! Benchmark harness shared by the `lumen-bench-*` binaries.

pub mod composition;
pub mod decode;
pub mod expression;
pub mod json_parse;
#[cfg(feature = "3d")]
pub mod scene_3d;
pub mod text;

mod fixtures;
mod media;
mod report;
mod timing;

/// A reproducible workload built in-process (not loaded from demo JSON).
trait CompositionFixture {
    fn name(&self) -> &'static str;
    fn build(&self) -> lumen_engine::composition::Composition;
    fn media_store(&self) -> media::BenchmarkMediaStore {
        media::BenchmarkMediaStore::default()
    }
    fn default_frames(&self, composition: &lumen_engine::composition::Composition) -> u32 {
        composition.timeline().duration_frames.min(90)
    }
    /// Whether the benchmark disables reuse of static node output, so every frame pays for its
    /// shaders and passes.
    fn full_render(&self) -> bool {
        false
    }
}
