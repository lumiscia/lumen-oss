use std::sync::{Arc, Mutex};

/// A frame's output. It stays valid for whatever the host submits next in queue order, and is
/// written again by a later frame.
#[derive(Debug, Clone)]
pub struct RasterHandle {
    pub texture: Arc<lumen_gpu::wgpu::Texture>,
    pub domain: lumen_gpu::TextureDomain,
}

/// Where a frame renders to.
#[derive(Debug, Clone, Default)]
pub enum RenderTarget {
    /// A texture of the renderer's, which a later frame writes again.
    #[default]
    Owned,
    /// A texture of the caller's, of the output's size and format, which stays with the caller:
    /// the renderer holds no claim on it once it is the only one left holding it.
    External(lumen_gpu::ExternalTexture),
}

/// The time a node spent binding, summed over a render.
#[derive(Debug, Clone, PartialEq)]
pub struct NodeTiming {
    pub node: crate::node::NodeId,
    /// The template the node is in, or `None` at the root. Node ids are only unique within one.
    pub template: Option<String>,
    /// The node's `type` in JSON.
    pub kind: &'static str,
    /// The node's own time: what it took, less what the nodes it evaluated inside it took.
    pub bind: std::time::Duration,
    /// How many frames evaluated the node. A frame the node is kept from, or rebinds, counts once.
    pub frames: u32,
}

/// How a camera projects, as `camera_3d` does.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CameraProjection {
    Perspective {
        /// The vertical field of view.
        fov_y_degrees: f32,
        near: f32,
        far: f32,
    },
    Orthographic {
        /// How many world units tall the view is.
        height: f32,
        near: f32,
        far: f32,
    },
}

/// A viewpoint for [`GpuCompositionRenderer::set_camera_override`](super::GpuCompositionRenderer::set_camera_override),
/// looking down its own -Z. The aspect is that of the `render_3d` it draws. It has no lens, so the
/// render is sharp throughout whatever the scene camera's `f_stop`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SceneCamera {
    /// Camera to world, as 16 floats in column-major order.
    pub world: [f32; 16],
    pub projection: CameraProjection,
}

/// A world-space axis-aligned box.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SceneBounds {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

/// An object a `render_3d` draws, as [`SceneCapture`] reports it.
#[derive(Debug, Clone, PartialEq)]
pub struct CapturedObject {
    /// The scene node that made the object. A model's primitives are objects of the `model_3d`.
    pub node: crate::node::NodeId,
    /// The template `node` is in, or `None` at the root.
    pub template: Option<String>,
    /// The clips that play the templates `node` is in, from the `render_3d`'s own graph inward;
    /// empty when `node` is in that graph. Instances of one template tell their nodes apart by
    /// these.
    pub clips: Vec<crate::node::NodeId>,
    /// Mesh to world, as 16 floats in column-major order.
    pub world: [f32; 16],
    /// What the object covers in world space, at least.
    pub bounds: SceneBounds,
}

/// A camera a `render_3d`'s scene holds, as [`SceneCapture`] reports it.
#[derive(Debug, Clone, PartialEq)]
pub struct CapturedCamera {
    /// The `camera_3d`, or the `model_3d` whose camera it is.
    pub node: crate::node::NodeId,
    /// The template `node` is in, or `None` at the root.
    pub template: Option<String>,
    /// The clips that play the templates `node` is in, as [`CapturedObject::clips`] says.
    pub clips: Vec<crate::node::NodeId>,
    /// Camera to world, as 16 floats in column-major order.
    pub world: [f32; 16],
    pub projection: CameraProjection,
    pub lens: CameraLens,
}

/// The lens of a camera [`SceneCapture`] reports, through which `render_3d` blurs what is nearer
/// or farther than its focus distance.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CameraLens {
    /// Diameter of the aperture in world units. 0 is a pinhole, which keeps everything sharp.
    pub aperture: f32,
    /// Distance along the view, in world units, that stays sharp.
    pub focus_distance: f32,
    /// Sides of the aperture: 0 is a circle.
    pub blades: u32,
    /// Turn of the aperture polygon in degrees, clockwise. At 0 one corner points up.
    pub rotation_degrees: f32,
}

/// What one `render_3d` placed on a frame, which
/// [`GpuCompositionRenderer::take_scene_captures`](super::GpuCompositionRenderer::take_scene_captures)
/// returns.
#[derive(Debug, Clone, PartialEq)]
pub struct SceneCapture {
    /// The `render_3d`.
    pub render: crate::node::NodeId,
    /// The template the `render_3d` is in, or `None` at the root. Node ids are only unique within
    /// one, and objects and cameras from scene templates say which template they are in.
    pub template: Option<String>,
    /// The objects the scene draws, in the order it walks them: one for each of the scene's nodes
    /// that makes a drawn object, and one for each copy of it that an `instance_3d` places, with
    /// the copy's own world matrix and bounds, even where copies are placed alike, and for each
    /// clip playing a scene template that holds the node. A
    /// `particles_3d` makes one object, whose bounds cover all its particles and whose world
    /// matrix places the particle system, whatever the objects it scatters are. An object drawn
    /// in several parts, such as a box with a material on each face, is reported once, and one
    /// with no extent is left out.
    pub objects: Vec<CapturedObject>,
    /// Every camera the scene holds, in the order it walks them. The first is the one the render
    /// draws through unless a camera override is set.
    pub cameras: Vec<CapturedCamera>,
}

/// A frame submitted to the GPU.
#[derive(Debug, Clone)]
pub struct RenderedFrame {
    /// The frame's output, in the target's texture.
    pub raster: RasterHandle,
    /// The submission that writes it.
    pub submission: lumen_gpu::wgpu::SubmissionIndex,
}

impl RenderedFrame {
    /// Blocks until the GPU has finished the frame.
    pub fn wait(&self, device: &lumen_gpu::wgpu::Device) -> crate::Result<()> {
        device
            .poll(lumen_gpu::wgpu::PollType::Wait {
                submission_index: Some(self.submission.clone()),
                timeout: None,
            })
            .map(drop)
            .map_err(|error| {
                crate::error::RenderError::Gpu {
                    details: format!("waiting for the frame failed: {error}"),
                }
                .into()
            })
    }
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
    /// The texture of the external target the frame was recorded for.
    pub(crate) external: Option<Arc<lumen_gpu::wgpu::Texture>>,
}

#[derive(Debug, Clone)]
struct AtlasRegion {
    data: Vec<u8>,
    origin: [u32; 3],
    size: lumen_gpu::Size,
}

/// Identifies the cached texture of an image: its pixels at the size they are decoded at.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MediaTextureKey {
    /// The image's identity, which changes when its pixels do.
    pub identity: crate::media::MediaIdentity,
    pub width: u32,
    pub height: u32,
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
                .map_err(|_| crate::error::RenderError::Internal {
                    details: "text atlas lock was poisoned".to_string(),
                })?
                .commit(self.text_atlas_frame);
        }
        Ok(())
    }

    /// How many regions text placed in the atlas this frame.
    #[cfg(feature = "test-hooks")]
    pub fn atlas_region_count(&self) -> usize {
        self.atlas_regions.len()
    }

    /// Where each region text placed in the atlas this frame goes, and its size.
    #[cfg(feature = "test-hooks")]
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
    #[cfg(feature = "test-hooks")]
    pub fn glyph_instances(&self) -> &[lumen_text::GpuGlyphInstance] {
        &self.glyph_instances
    }
}
