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

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FramePortRef {
    pub port: PortRef,
    pub frame: u32,
}

impl FramePortRef {
    pub fn new(port: PortRef, frame: u32) -> Self {
        Self { port, frame }
    }
}

pub trait GpuCompiledNode: std::fmt::Debug + Send + Sync {
    fn node_id(&self) -> NodeId;

    fn bind(
        &self,
        ctx: &crate::gpu::FrameBindContext<'_>,
        bound: &mut BoundFrame,
    ) -> crate::Result<()>;

    fn invalidate_gpu_resources(&self) {}
}

#[derive(Debug)]
pub struct CompiledComposition {
    pub plan: lumen_gpu::RenderPlan,
    pub output: RasterHandle,
    pub node_outputs: HashMap<PortRef, CompiledOutput>,
    pub compiled_nodes: HashMap<NodeId, Box<dyn GpuCompiledNode>>,
}

#[derive(Debug, Clone, Default)]
pub struct BoundFrame {
    buffer_uploads: Vec<(lumen_gpu::BufferId, u64, Vec<u8>)>,
    texture_uploads: Vec<TextureUpload>,
    media_textures: Vec<MediaTextureUpload>,
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
    Rgba16FloatRegion {
        id: lumen_gpu::TextureId,
        data: Vec<u16>,
        origin: [u32; 3],
        size: lumen_gpu::Size,
        bytes_per_row: u32,
        rows_per_image: u32,
    },
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
    pub frame: MediaFrame,
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

    pub fn write_texture_rgba16_float_region(
        &mut self,
        id: lumen_gpu::TextureId,
        data: impl Into<Vec<u16>>,
        origin: [u32; 3],
        size: lumen_gpu::Size,
        bytes_per_row: u32,
        rows_per_image: u32,
    ) {
        self.texture_uploads.push(TextureUpload::Rgba16FloatRegion {
            id,
            data: data.into(),
            origin,
            size,
            bytes_per_row,
            rows_per_image,
        });
    }

    pub fn use_media_texture(
        &mut self,
        texture: lumen_gpu::TextureId,
        key: MediaTextureKey,
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
        for (id, offset, data) in &self.buffer_uploads {
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
                TextureUpload::Rgba16FloatRegion {
                    id,
                    data,
                    origin,
                    size,
                    bytes_per_row,
                    rows_per_image,
                } => {
                    update.write_texture_rgba16_float_region(
                        *id,
                        data,
                        *origin,
                        *size,
                        *bytes_per_row,
                        *rows_per_image,
                    );
                }
            }
        }
        update
    }
}
