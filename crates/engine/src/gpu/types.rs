use std::sync::{Arc, Mutex};

/// A frame's output. It stays valid for whatever the host submits next in queue order, and is
/// written again by a later frame.
#[derive(Debug, Clone)]
pub struct RasterHandle {
    pub texture: Arc<lumen_gpu::wgpu::Texture>,
    pub domain: lumen_gpu::TextureDomain,
}

pub(super) const TEXT_ATLAS_SIZE: u32 = 4096;

pub(super) fn create_text_atlas_texture(
    device: &lumen_gpu::wgpu::Device,
) -> lumen_gpu::wgpu::Texture {
    let size = lumen_gpu::Size::new(TEXT_ATLAS_SIZE, TEXT_ATLAS_SIZE);
    let desc = lumen_gpu::TextureDesc::sampled(size, lumen_gpu::wgpu::TextureFormat::Rgba8Unorm);
    device.create_texture(&lumen_gpu::wgpu::TextureDescriptor {
        label: Some("lumen shared glyph atlas"),
        size: size.as_extent(),
        mip_level_count: 1,
        sample_count: 1,
        dimension: lumen_gpu::wgpu::TextureDimension::D2,
        format: desc.format,
        usage: desc.usage,
        view_formats: &[],
    })
}

/// A recorded frame, waiting to be uploaded and submitted.
#[derive(Debug, Clone, Default)]
pub struct BoundFrame {
    /// Which recording this was, so that only the renderer's latest is uploaded.
    pub(crate) sequence: u64,
    atlas_regions: Vec<AtlasRegion>,
    glyph_instances: Vec<lumen_text::GpuGlyphInstance>,
    pub(crate) text_atlas: Option<Arc<Mutex<lumen_text::GlyphAtlas>>>,
    pub(crate) text_atlas_frame: u64,
    /// Whether text dropped glyphs that a later frame with more atlas room may draw.
    pub(crate) text_dropped_glyphs: bool,
}

#[derive(Debug, Clone)]
struct AtlasRegion {
    data: Vec<u8>,
    origin: [u32; 3],
    size: lumen_gpu::Size,
}

/// Identifies the cached texture of an image.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MediaTextureKey {
    /// The image's identity, which changes when its pixels do.
    pub identity: crate::media::MediaIdentity,
    pub width: u32,
    pub height: u32,
}

impl MediaTextureKey {
    /// The key of the texture an image is drawn from at its own size.
    pub(crate) fn of_image(
        identity: crate::media::MediaIdentity,
        resolver: &dyn crate::media::ImageResolver,
    ) -> Self {
        let metadata = resolver.metadata();
        Self {
            identity,
            width: metadata.width.max(1),
            height: metadata.height.max(1),
        }
    }
}

impl BoundFrame {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Adds a region of tightly packed RGBA8 that text placed in the atlas this frame.
    pub(crate) fn add_atlas_region(
        &mut self,
        data: Vec<u8>,
        origin: [u32; 2],
        size: lumen_gpu::Size,
    ) {
        self.atlas_regions.push(AtlasRegion {
            data,
            origin: [origin[0], origin[1], 0],
            size,
        });
    }

    pub(crate) fn add_glyph_instances(&mut self, instances: &[lumen_text::GpuGlyphInstance]) {
        self.glyph_instances.extend_from_slice(instances);
    }

    /// Writes the atlas regions text placed this frame into `atlas`.
    pub(crate) fn write_atlas_regions(
        &self,
        queue: &lumen_gpu::wgpu::Queue,
        atlas: &lumen_gpu::wgpu::Texture,
    ) {
        for AtlasRegion { data, origin, size } in &self.atlas_regions {
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
                    bytes_per_row: Some(size.width * 4),
                    rows_per_image: Some(size.height),
                },
                size.as_extent(),
            );
        }
    }

    /// Commits the frame's glyphs to the atlas, once its regions are written.
    pub(crate) fn mark_uploaded(&self) -> crate::Result<()> {
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

    /// How many regions text placed in the atlas this frame.
    #[doc(hidden)]
    pub fn atlas_region_count(&self) -> usize {
        self.atlas_regions.len()
    }

    /// Where each region text placed in the atlas this frame goes, and its size.
    #[doc(hidden)]
    pub fn atlas_region_rects(&self) -> Vec<([u32; 2], [u32; 2])> {
        self.atlas_regions
            .iter()
            .map(|region| {
                (
                    [region.origin[0], region.origin[1]],
                    [region.size.width, region.size.height],
                )
            })
            .collect()
    }

    /// The glyph, stroke and decoration instances the frame draws, in draw order per text.
    #[doc(hidden)]
    pub fn glyph_instances(&self) -> &[lumen_text::GpuGlyphInstance] {
        &self.glyph_instances
    }
}
