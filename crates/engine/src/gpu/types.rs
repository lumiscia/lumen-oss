use std::collections::HashMap;

use crate::media::MediaFrame;
use crate::node::{NodeId, PortRef};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlphaMode {
    Premultiplied,
    Unpremultiplied,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorSpace {
    Srgb,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RasterMetadata {
    pub alpha_mode: AlphaMode,
    pub color_space: ColorSpace,
}

impl Default for RasterMetadata {
    fn default() -> Self {
        Self {
            alpha_mode: AlphaMode::Premultiplied,
            color_space: ColorSpace::Srgb,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RasterHandle {
    pub texture: lumen_gpu::TextureId,
    pub domain: lumen_gpu::TextureDomain,
    pub metadata: RasterMetadata,
    /// Uniform holding the raster's logical bounds (`origin: vec2<i32>`, `size: vec2<u32>`)
    /// within its storage texture, or `None` when it fills the whole texture.
    pub bounds: Option<lumen_gpu::BufferId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompiledOutput {
    Raster(RasterHandle),
    Empty,
}

impl CompiledOutput {
    pub fn into_raster(self, node_id: NodeId, _port: &str) -> crate::Result<RasterHandle> {
        match self {
            Self::Raster(raster) => Ok(raster),
            Self::Empty => Err(crate::error::RenderError::InvalidNodeOutputType {
                frame: 0,
                node_id,
                expected: "Raster",
                actual: "Empty",
            }
            .into()),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct FramePortRef {
    pub port: PortRef,
    pub frame: f64,
}

impl FramePortRef {
    pub fn new(port: PortRef, frame: f64) -> Self {
        Self { port, frame }
    }
}

/// TimeRemap nodes traversed from the media output to reach a compiled node instance.
///
/// A node reached through different remap paths is compiled once per path, so the path (not the
/// absolute frame) identifies its resources across every frame bound to the same plan.
pub type TimeRemapPath = Vec<NodeId>;

pub trait GpuCompiledNode: std::fmt::Debug + Send + Sync {
    fn node_id(&self) -> NodeId;

    fn bind(
        &self,
        ctx: &crate::gpu::FrameBindContext<'_>,
        bound: &mut BoundFrame,
    ) -> crate::Result<()>;

    /// Binds, for a node whose output is kept from an earlier frame, the shared state that nodes
    /// bound after it see, as `bind` would, without writing the node's own resources. Returns
    /// `true` when the kept output no longer matches what `bind` would render, having then bound
    /// the node fully.
    fn bind_kept(
        &self,
        _ctx: &crate::gpu::FrameBindContext<'_>,
        _bound: &mut BoundFrame,
    ) -> crate::Result<bool> {
        Ok(false)
    }
}

#[derive(Debug)]
pub struct CompiledComposition {
    pub plan: lumen_gpu::RenderPlan,
    pub(crate) cache: super::cache::CacheLayout,
    pub(crate) text_atlas_texture: Option<lumen_gpu::TextureId>,
    pub(crate) text_atlas: std::sync::Arc<std::sync::Mutex<lumen_text::GlyphAtlas>>,
    pub output: RasterHandle,
    pub node_outputs: HashMap<PortRef, CompiledOutput>,
    pub compiled_nodes: HashMap<NodeId, HashMap<TimeRemapPath, Box<dyn GpuCompiledNode>>>,
}

impl CompiledComposition {
    /// Installs the 4096² RGBA8 text atlas (64 MiB) after `Renderer::prepare_plan`.
    /// Standalone callers must call this before uploading text; repeated calls retain the atlas.
    /// `GpuCompositionRenderer` installs its shared atlas automatically.
    pub fn prepare_text_atlas(&self, renderer: &mut lumen_gpu::Renderer) -> crate::Result<()> {
        let Some(id) = self.text_atlas_texture else {
            return Ok(());
        };
        let desc = text_atlas_desc();
        if renderer.texture_desc(id) == Some(desc) {
            return Ok(());
        }
        renderer
            .replace_texture(id, create_text_atlas_texture(&renderer.device), desc)
            .map_err(|error| crate::error::RenderError::Gpu {
                details: error.to_string(),
            })?
            .destroy();
        Ok(())
    }
}

pub(super) const TEXT_ATLAS_SIZE: u32 = 4096;

pub(super) fn text_atlas_desc() -> lumen_gpu::TextureDesc {
    lumen_gpu::TextureDesc::sampled(
        lumen_gpu::Size::new(TEXT_ATLAS_SIZE, TEXT_ATLAS_SIZE),
        lumen_gpu::wgpu::TextureFormat::Rgba8Unorm,
    )
}

pub(super) fn create_text_atlas_texture(
    device: &lumen_gpu::wgpu::Device,
) -> lumen_gpu::wgpu::Texture {
    let desc = text_atlas_desc();
    device.create_texture(&lumen_gpu::wgpu::TextureDescriptor {
        label: Some("lumen shared glyph atlas"),
        size: desc.domain.storage_size.as_extent(),
        mip_level_count: 1,
        sample_count: 1,
        dimension: lumen_gpu::wgpu::TextureDimension::D2,
        format: desc.format,
        usage: desc.usage,
        view_formats: &[],
    })
}

#[derive(Debug, Clone, Default)]
pub struct BoundFrame {
    pub(crate) renderer_binding: Option<(u64, super::renderer::CompiledPlanKey)>,
    /// Glyph instances of a recorded frame, already copied to its bump buffer.
    recorded_uploads: Vec<(lumen_gpu::BufferId, u64, Vec<u8>)>,
    /// The bind sequence of a frame recorded rather than bound to a plan.
    pub(crate) recorded: Option<u64>,
    /// Which nodes were bound, when bound against a renderer's kept output.
    pub(crate) cache: Option<super::cache::BoundCache>,
    buffer_uploads: Vec<(lumen_gpu::BufferId, u64, Vec<u8>)>,
    texture_uploads: Vec<TextureUpload>,
    media_textures: Vec<MediaTextureUpload>,
    pub(crate) text_atlas: Option<std::sync::Arc<std::sync::Mutex<lumen_text::GlyphAtlas>>>,
    pub(crate) text_atlas_frame: u64,
    pub(crate) text_generation_count: usize,
    pub(crate) text_generation_ms: f64,
    /// Whether text dropped glyphs that a later frame with more atlas room may draw.
    pub(crate) text_dropped_glyphs: bool,
}

#[derive(Debug, Clone)]
enum TextureUpload {
    Rgba8 {
        id: lumen_gpu::TextureId,
        data: Vec<u8>,
        bytes_per_row: u32,
        rows_per_image: u32,
    },
    Rgba8Region {
        id: lumen_gpu::TextureId,
        data: Vec<u8>,
        origin: [u32; 3],
        size: lumen_gpu::Size,
        bytes_per_row: u32,
        rows_per_image: u32,
    },
    Rgba16Float {
        id: lumen_gpu::TextureId,
        data: Vec<u16>,
        bytes_per_row: u32,
        rows_per_image: u32,
    },
}

/// Identifies the cached texture of an image.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MediaTextureKey {
    /// The image's resolver ID, which changes when its pixels do.
    pub source: String,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone)]
pub struct MediaTextureUpload {
    pub texture: lumen_gpu::TextureId,
    /// `None` for a video frame, which is never cached.
    pub key: Option<MediaTextureKey>,
    pub frame: MediaFrame,
    pub size: lumen_gpu::Size,
}

impl BoundFrame {
    pub fn new() -> Self {
        Self::default()
    }

    /// Takes the buffer writes so far, for a recorded frame's bump buffers.
    pub(crate) fn take_buffer_uploads(&mut self) -> Vec<(lumen_gpu::BufferId, u64, Vec<u8>)> {
        std::mem::take(&mut self.buffer_uploads)
    }

    pub(crate) fn keep_recorded_uploads(
        &mut self,
        uploads: impl IntoIterator<Item = (lumen_gpu::BufferId, u64, Vec<u8>)>,
    ) {
        self.recorded_uploads.extend(uploads);
    }

    /// Writes the atlas regions text placed this frame into `atlas`.
    pub(crate) fn write_atlas_regions(
        &self,
        queue: &lumen_gpu::wgpu::Queue,
        atlas: &lumen_gpu::wgpu::Texture,
    ) {
        for upload in &self.texture_uploads {
            let TextureUpload::Rgba8Region {
                data,
                origin,
                size,
                bytes_per_row,
                rows_per_image,
                ..
            } = upload
            else {
                continue;
            };
            queue.write_texture(
                lumen_gpu::wgpu::TexelCopyTextureInfo {
                    texture: atlas,
                    mip_level: 0,
                    origin: lumen_gpu::wgpu::Origin3d {
                        x: origin[0],
                        y: origin[1],
                        z: origin[2],
                    },
                    aspect: lumen_gpu::wgpu::TextureAspect::All,
                },
                data,
                lumen_gpu::wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(*bytes_per_row),
                    rows_per_image: Some(*rows_per_image),
                },
                size.as_extent(),
            );
        }
    }

    pub fn write_buffer(&mut self, id: lumen_gpu::BufferId, offset: u64, data: impl Into<Vec<u8>>) {
        self.buffer_uploads.push((id, offset, data.into()));
    }

    pub fn write_texture_rgba8(
        &mut self,
        id: lumen_gpu::TextureId,
        data: impl Into<Vec<u8>>,
        bytes_per_row: u32,
        rows_per_image: u32,
    ) {
        self.texture_uploads.push(TextureUpload::Rgba8 {
            id,
            data: data.into(),
            bytes_per_row,
            rows_per_image,
        });
    }

    pub fn write_texture_rgba8_region(
        &mut self,
        id: lumen_gpu::TextureId,
        data: impl Into<Vec<u8>>,
        origin: [u32; 3],
        size: lumen_gpu::Size,
        bytes_per_row: u32,
        rows_per_image: u32,
    ) {
        self.texture_uploads.push(TextureUpload::Rgba8Region {
            id,
            data: data.into(),
            origin,
            size,
            bytes_per_row,
            rows_per_image,
        });
    }

    pub fn write_texture_rgba16_float(
        &mut self,
        id: lumen_gpu::TextureId,
        data: impl Into<Vec<u16>>,
        bytes_per_row: u32,
        rows_per_image: u32,
    ) {
        self.texture_uploads.push(TextureUpload::Rgba16Float {
            id,
            data: data.into(),
            bytes_per_row,
            rows_per_image,
        });
    }

    pub fn use_media_texture(
        &mut self,
        texture: lumen_gpu::TextureId,
        key: Option<MediaTextureKey>,
        frame: MediaFrame,
        size: lumen_gpu::Size,
    ) {
        self.media_textures.push(MediaTextureUpload {
            texture,
            key,
            frame,
            size,
        });
    }

    /// Call only after uploading the latest bind to its compiled plan.
    pub fn mark_uploaded(&self) -> crate::Result<()> {
        if let Some(atlas) = &self.text_atlas {
            atlas
                .lock()
                .map_err(|_| crate::error::RenderError::Gpu {
                    details: "text atlas lock was poisoned".to_string(),
                })?
                .commit(self.text_atlas_frame);
        }
        Ok(())
    }

    pub fn media_textures(&self) -> &[MediaTextureUpload] {
        &self.media_textures
    }

    pub fn buffer_upload_count(&self) -> usize {
        self.buffer_uploads.len()
    }

    pub fn texture_upload_count(&self) -> usize {
        self.texture_uploads.len()
    }

    pub fn frame_update(&self) -> lumen_gpu::FrameUpdate<'_> {
        let mut update = lumen_gpu::FrameUpdate::new();
        for (id, offset, data) in self.buffer_uploads.iter().chain(&self.recorded_uploads) {
            update.write_buffer(*id, *offset, data);
        }
        for upload in &self.texture_uploads {
            match upload {
                TextureUpload::Rgba8 {
                    id,
                    data,
                    bytes_per_row,
                    rows_per_image,
                } => {
                    update.write_texture_rgba8(*id, data, *bytes_per_row, *rows_per_image);
                }
                TextureUpload::Rgba8Region {
                    id,
                    data,
                    origin,
                    size,
                    bytes_per_row,
                    rows_per_image,
                } => {
                    update.write_texture_rgba8_region(
                        *id,
                        data,
                        *origin,
                        *size,
                        *bytes_per_row,
                        *rows_per_image,
                    );
                }
                TextureUpload::Rgba16Float {
                    id,
                    data,
                    bytes_per_row,
                    rows_per_image,
                } => {
                    update.write_texture_rgba16_float(*id, data, *bytes_per_row, *rows_per_image);
                }
            }
        }
        update
    }
}
