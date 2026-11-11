//! The byte budgets of what a renderer keeps of media and derived GPU data between frames, and
//! what it holds.

/// How many bytes each of a renderer's caches keeps of what the latest frame doesn't use. What the
/// frame being rendered uses, and what a kept output was drawn from, stays whatever its budget;
/// past it, the least recently used of the rest are freed. Every figure is GPU memory, estimated
/// from texture sizes and buffer lengths. Set with
/// [`super::GpuCompositionRenderer::set_cache_budgets`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CacheBudgets {
    /// Image textures no frame binds. 256 MiB.
    pub image_textures: u64,
    /// Pooled textures frames are done with, which the next frames reuse. 384 MiB.
    pub pooled_textures: u64,
    /// Outputs of nodes that don't change with the frame, kept so later frames skip drawing
    /// them. Each is also dropped after 600 frames unread. 1 GiB.
    pub retained_outputs: u64,
    /// `.cube` colour lookup tables. 128 MiB.
    pub luts: u64,
    /// 3D models' buffers and textures. 256 MiB.
    pub models: u64,
    /// 3D environment maps. 256 MiB.
    pub environments: u64,
    /// Extruded `text_3d` meshes, of which at most 256 are kept. 64 MiB.
    pub text_meshes: u64,
}

impl Default for CacheBudgets {
    fn default() -> Self {
        Self {
            image_textures: 256 << 20,
            pooled_textures: lumen_gpu::DEFAULT_POOL_BUDGET_BYTES,
            retained_outputs: 1 << 30,
            luts: 128 << 20,
            models: 256 << 20,
            environments: 256 << 20,
            text_meshes: 64 << 20,
        }
    }
}

/// The bytes each of a renderer's caches holds, including what the latest frame uses, as
/// [`super::GpuCompositionRenderer::cache_stats`] reports them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CacheStats {
    pub image_textures: u64,
    /// Every pooled texture, those that retained outputs and the latest frame hold included.
    pub pooled_textures: u64,
    pub retained_outputs: u64,
    pub luts: u64,
    pub models: u64,
    pub environments: u64,
    pub text_meshes: u64,
    /// The shared glyph atlas, 64 MiB once a frame has drawn text, which has no budget.
    pub text_atlas: u64,
}
