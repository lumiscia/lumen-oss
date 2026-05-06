use std::{collections::HashMap, sync::Arc};

use crate::media::CpuMediaFrame;
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

#[derive(Debug, Clone)]
pub enum FrameBinding {
    SolidColor {
        node_id: NodeId,
        color: crate::node::NodeProperty,
        buffer: lumen_gpu::BufferId,
    },
    Shape {
        node_id: NodeId,
        geometry_kind: crate::node::NodeProperty,
        width: crate::node::NodeProperty,
        height: crate::node::NodeProperty,
        border_radius: crate::node::NodeProperty,
        position: crate::node::NodeProperty,
        fill_enabled: crate::node::NodeProperty,
        fill_color: crate::node::NodeProperty,
        stroke_enabled: crate::node::NodeProperty,
        stroke_color: crate::node::NodeProperty,
        stroke_width: crate::node::NodeProperty,
        buffer: lumen_gpu::BufferId,
    },
    Text {
        node_id: NodeId,
        content: crate::node::NodeProperty,
        font_family: crate::node::NodeProperty,
        font_size: crate::node::NodeProperty,
        font_weight: crate::node::NodeProperty,
        font_style: crate::node::NodeProperty,
        max_width: crate::node::NodeProperty,
        position: crate::node::NodeProperty,
        color: crate::node::NodeProperty,
        alignment_horizontal: crate::node::NodeProperty,
        alignment_vertical: crate::node::NodeProperty,
        output_texture: lumen_gpu::TextureId,
        atlas_texture: lumen_gpu::TextureId,
        globals_buffer: lumen_gpu::BufferId,
        instances_buffer: lumen_gpu::BufferId,
        atlas_size: lumen_gpu::Size,
        max_glyphs: usize,
        size: lumen_gpu::Size,
    },
    Path {
        node_id: NodeId,
        data: crate::node::NodeProperty,
        position: crate::node::NodeProperty,
        fill_enabled: crate::node::NodeProperty,
        fill_color: crate::node::NodeProperty,
        stroke_enabled: crate::node::NodeProperty,
        stroke_color: crate::node::NodeProperty,
        stroke_width: crate::node::NodeProperty,
        params_buffer: lumen_gpu::BufferId,
        points_buffer: lumen_gpu::BufferId,
        max_points: usize,
    },
    AlphaPremultiply {
        node_id: NodeId,
        mode: crate::node::NodeProperty,
        buffer: lumen_gpu::BufferId,
    },
    ChannelShuffle {
        node_id: NodeId,
        red: crate::node::NodeProperty,
        green: crate::node::NodeProperty,
        blue: crate::node::NodeProperty,
        alpha: crate::node::NodeProperty,
        buffer: lumen_gpu::BufferId,
    },
    ColorGrade {
        node_id: NodeId,
        lut_source: crate::node::NodeProperty,
        strength: crate::node::NodeProperty,
        interpolation: crate::node::NodeProperty,
        params_buffer: lumen_gpu::BufferId,
        lut_buffer: lumen_gpu::BufferId,
    },
    Exposure {
        node_id: NodeId,
        exposure: crate::node::NodeProperty,
        contrast: crate::node::NodeProperty,
        offset: crate::node::NodeProperty,
        buffer: lumen_gpu::BufferId,
    },
    HueSaturation {
        node_id: NodeId,
        hue_degrees: crate::node::NodeProperty,
        saturation: crate::node::NodeProperty,
        lightness: crate::node::NodeProperty,
        buffer: lumen_gpu::BufferId,
    },
    Levels {
        node_id: NodeId,
        black_point: crate::node::NodeProperty,
        white_point: crate::node::NodeProperty,
        gamma: crate::node::NodeProperty,
        output_black: crate::node::NodeProperty,
        output_white: crate::node::NodeProperty,
        buffer: lumen_gpu::BufferId,
    },
    Blur {
        node_id: NodeId,
        radius: crate::node::NodeProperty,
        buffer: lumen_gpu::BufferId,
    },
    Curves {
        node_id: NodeId,
        curve_source: crate::node::NodeProperty,
        strength: crate::node::NodeProperty,
        params_buffer: lumen_gpu::BufferId,
        curve_buffer: lumen_gpu::BufferId,
    },
    Shadow {
        node_id: NodeId,
        offset_x: crate::node::NodeProperty,
        offset_y: crate::node::NodeProperty,
        radius: crate::node::NodeProperty,
        color: crate::node::NodeProperty,
        opacity: crate::node::NodeProperty,
        buffer: lumen_gpu::BufferId,
    },
    WgslShader {
        node_id: NodeId,
        shader: crate::node::NodeProperty,
        bindings: crate::node::NodeProperty,
        buffer: lumen_gpu::BufferId,
    },
    Boolean {
        node_id: NodeId,
        operation: crate::node::NodeProperty,
        threshold: crate::node::NodeProperty,
        buffer: lumen_gpu::BufferId,
    },
    RasterMultiMerge {
        node_id: NodeId,
        opacity: crate::node::NodeProperty,
        blend_mode: crate::node::NodeProperty,
        buffer: lumen_gpu::BufferId,
    },
    Merge {
        node_id: NodeId,
        opacity: crate::node::NodeProperty,
        blend_mode: crate::node::NodeProperty,
        has_mask: bool,
        buffer: lumen_gpu::BufferId,
    },
    Memo {
        node_id: NodeId,
        cache_id: crate::node::NodeProperty,
        allow_expressions: crate::node::NodeProperty,
    },
    TimeRemap {
        node_id: NodeId,
        frame: crate::node::NodeProperty,
        loop_enabled: crate::node::NodeProperty,
        loop_start: crate::node::NodeProperty,
        loop_end: crate::node::NodeProperty,
    },
    Transform {
        node_id: NodeId,
        scale_x: crate::node::NodeProperty,
        scale_y: crate::node::NodeProperty,
        translate_x: crate::node::NodeProperty,
        translate_y: crate::node::NodeProperty,
        rotate: crate::node::NodeProperty,
        pivot_x: crate::node::NodeProperty,
        pivot_y: crate::node::NodeProperty,
        sampling: crate::node::NodeProperty,
        buffer: lumen_gpu::BufferId,
    },
    Crop {
        node_id: NodeId,
        x: crate::node::NodeProperty,
        y: crate::node::NodeProperty,
        width: crate::node::NodeProperty,
        height: crate::node::NodeProperty,
        buffer: lumen_gpu::BufferId,
    },
    Resize {
        node_id: NodeId,
        width: crate::node::NodeProperty,
        height: crate::node::NodeProperty,
        mode: crate::node::NodeProperty,
        sampling: crate::node::NodeProperty,
        buffer: lumen_gpu::BufferId,
    },
    Switch {
        node_id: NodeId,
        selected_layer: Option<usize>,
    },
    MediaInput {
        node_id: NodeId,
        source: crate::node::NodeProperty,
        texture: lumen_gpu::TextureId,
        size: lumen_gpu::Size,
    },
}

impl FrameBinding {
    pub fn node_id(&self) -> NodeId {
        match self {
            Self::SolidColor { node_id, .. }
            | Self::Shape { node_id, .. }
            | Self::Text { node_id, .. }
            | Self::Path { node_id, .. }
            | Self::AlphaPremultiply { node_id, .. }
            | Self::ChannelShuffle { node_id, .. }
            | Self::ColorGrade { node_id, .. }
            | Self::Exposure { node_id, .. }
            | Self::HueSaturation { node_id, .. }
            | Self::Levels { node_id, .. }
            | Self::Blur { node_id, .. }
            | Self::Curves { node_id, .. }
            | Self::Shadow { node_id, .. }
            | Self::WgslShader { node_id, .. }
            | Self::Boolean { node_id, .. }
            | Self::RasterMultiMerge { node_id, .. }
            | Self::Merge { node_id, .. }
            | Self::Memo { node_id, .. }
            | Self::TimeRemap { node_id, .. }
            | Self::Transform { node_id, .. }
            | Self::Crop { node_id, .. }
            | Self::Resize { node_id, .. }
            | Self::Switch { node_id, .. }
            | Self::MediaInput { node_id, .. } => *node_id,
        }
    }
}

#[derive(Debug, Clone)]
pub struct CompiledComposition {
    pub plan: lumen_gpu::RenderPlan,
    pub output: RasterHandle,
    pub node_outputs: HashMap<PortRef, CompiledOutput>,
    pub frame_bindings: Vec<FrameBinding>,
    pub frame_binding_frames: Vec<Option<u32>>,
}

#[derive(Debug, Clone, Default)]
pub struct BoundFrame {
    buffer_uploads: Vec<(lumen_gpu::BufferId, u64, Vec<u8>)>,
    texture_uploads: Vec<(lumen_gpu::TextureId, Vec<u8>, u32, u32)>,
    media_textures: Vec<MediaTextureUpload>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MediaTextureKey {
    pub source: String,
    pub frame: Option<u32>,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone)]
pub struct MediaTextureUpload {
    pub texture: lumen_gpu::TextureId,
    pub key: MediaTextureKey,
    pub frame: Arc<CpuMediaFrame>,
    pub size: lumen_gpu::Size,
}

impl BoundFrame {
    pub fn new() -> Self {
        Self::default()
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
        self.texture_uploads
            .push((id, data.into(), bytes_per_row, rows_per_image));
    }

    pub fn use_media_texture(
        &mut self,
        texture: lumen_gpu::TextureId,
        key: MediaTextureKey,
        frame: Arc<CpuMediaFrame>,
        size: lumen_gpu::Size,
    ) {
        self.media_textures.push(MediaTextureUpload {
            texture,
            key,
            frame,
            size,
        });
    }

    pub fn media_textures(&self) -> &[MediaTextureUpload] {
        &self.media_textures
    }

    pub fn frame_update(&self) -> lumen_gpu::FrameUpdate<'_> {
        let mut update = lumen_gpu::FrameUpdate::new();
        for (id, offset, data) in &self.buffer_uploads {
            update.write_buffer(*id, *offset, data);
        }
        for (id, data, bytes_per_row, rows_per_image) in &self.texture_uploads {
            update.write_texture_rgba8(*id, data, *bytes_per_row, *rows_per_image);
        }
        update
    }
}
