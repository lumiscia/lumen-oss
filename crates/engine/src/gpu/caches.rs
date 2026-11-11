//! The byte budgets of what a renderer keeps of media and derived GPU data between frames, and
//! what it holds.

/// The byte budget of each of a renderer's caches, checked as frames finish (image textures also
/// when the budget is set). Past its budget a cache frees entries, least recently used first,
/// until it is back within it or only protected entries are left. Every budget but
/// `image_textures` counts the cache's total bytes, protected entries included, so entries a
/// frame uses can push it past its budget and free every idle one. Figures are estimated from
/// texture sizes and buffer lengths. Set with
/// [`super::GpuCompositionRenderer::set_cache_budgets`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CacheBudgets {
    /// Image textures no frame binds. Only those count toward it; bound ones are kept. 256 MiB.
    pub image_textures: u64,
    /// Every pooled texture: those the latest frame used, retained outputs, and those frames are
    /// done with, which later frames reuse. Only the last are freed, and by size bucket: the
    /// bucket whose least recently used spare is oldest gives up its spares first, so the order is
    /// not least recently used across buckets. 384 MiB.
    pub pooled_textures: u64,
    /// Outputs of nodes that don't change with the frame, kept so later frames skip drawing them,
    /// counted in total. Those the latest frame read are kept. Each is also dropped after 600
    /// frames unread. 1 GiB.
    pub retained_outputs: u64,
    /// `.cube` colour lookup tables, counted in total. Those the latest frame used are kept.
    /// 128 MiB.
    pub luts: u64,
    /// Mip chains of the sources `transform` and `resize` show smaller than their size, and of
    /// the rasters 3D scenes sample, counted in total. Those the latest frame used are kept. Each
    /// is also dropped after 600 frames unused. 256 MiB.
    pub mip_chains: u64,
    /// 3D models, counted in total: each one's GPU buffers and textures, plus the CPU bounds of
    /// its primitives and joints kept beside them. Those the latest frame drew, and those a kept
    /// output was drawn from, are kept. 256 MiB.
    pub models: u64,
    /// 3D environment maps' textures, counted in total, kept as models are. 256 MiB.
    pub environments: u64,
    /// Extruded `text_3d` meshes' GPU buffers, counted in total, kept as models are. At most 256
    /// meshes are kept. 64 MiB.
    pub text_meshes: u64,
}

impl Default for CacheBudgets {
    fn default() -> Self {
        Self {
            image_textures: 256 << 20,
            pooled_textures: lumen_gpu::DEFAULT_POOL_BUDGET_BYTES,
            retained_outputs: 1 << 30,
            luts: 128 << 20,
            mip_chains: 256 << 20,
            models: 256 << 20,
            environments: 256 << 20,
            text_meshes: 64 << 20,
        }
    }
}

/// The bytes each of a renderer's caches holds, including what the latest frame uses, counted as
/// [`CacheBudgets`] counts them, as [`super::GpuCompositionRenderer::cache_stats`] reports them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CacheStats {
    pub image_textures: u64,
    /// Every pooled texture, those that retained outputs and the latest frame hold included.
    pub pooled_textures: u64,
    pub retained_outputs: u64,
    pub luts: u64,
    pub mip_chains: u64,
    /// GPU buffers and textures of 3D models, and the CPU bounds kept beside them.
    pub models: u64,
    /// Environment maps' textures, without the fixed 8 KiB lookup table their lighting uses.
    pub environments: u64,
    pub text_meshes: u64,
    /// The shared glyph atlas, 64 MiB once a frame has drawn text, which has no budget.
    pub text_atlas: u64,
}
