use std::marker::PhantomData;

use crate::{BufferId, NodeKey, TextureId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DynamicResource {
    Texture(TextureId),
    Buffer(BufferId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ParamKey {
    pub owner: NodeKey,
    pub slot: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParamTarget {
    Buffer(BufferId),
    Texture(TextureId),
}

#[derive(Debug, Clone)]
pub struct ParamSlot {
    pub key: ParamKey,
    pub target: ParamTarget,
}

#[derive(Debug, Clone)]
pub enum Upload<'a> {
    Buffer {
        id: BufferId,
        offset: u64,
        data: &'a [u8],
    },
    TextureRgba8 {
        id: TextureId,
        data: &'a [u8],
        bytes_per_row: u32,
        rows_per_image: u32,
    },
    TextureRgba8Region {
        id: TextureId,
        data: &'a [u8],
        origin: [u32; 3],
        size: crate::Size,
        bytes_per_row: u32,
        rows_per_image: u32,
    },
    TextureRgba16Float {
        id: TextureId,
        data: &'a [u16],
        bytes_per_row: u32,
        rows_per_image: u32,
    },
    TextureRgba16FloatRegion {
        id: TextureId,
        data: &'a [u16],
        origin: [u32; 3],
        size: crate::Size,
        bytes_per_row: u32,
        rows_per_image: u32,
    },
}

#[derive(Debug, Clone, Default)]
pub struct FrameUpdate<'a> {
    pub(crate) uploads: Vec<Upload<'a>>,
    _marker: PhantomData<&'a ()>,
}

impl<'a> FrameUpdate<'a> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn uploads(&self) -> &[Upload<'a>] {
        &self.uploads
    }

    pub fn write_buffer(&mut self, id: BufferId, offset: u64, data: &'a [u8]) -> &mut Self {
        self.uploads.push(Upload::Buffer { id, offset, data });
        self
    }

    pub fn write_texture_rgba8(
        &mut self,
        id: TextureId,
        data: &'a [u8],
        bytes_per_row: u32,
        rows_per_image: u32,
    ) -> &mut Self {
        self.uploads.push(Upload::TextureRgba8 {
            id,
            data,
            bytes_per_row,
            rows_per_image,
        });
        self
    }

    pub fn write_texture_rgba8_region(
        &mut self,
        id: TextureId,
        data: &'a [u8],
        origin: [u32; 3],
        size: crate::Size,
        bytes_per_row: u32,
        rows_per_image: u32,
    ) -> &mut Self {
        self.uploads.push(Upload::TextureRgba8Region {
            id,
            data,
            origin,
            size,
            bytes_per_row,
            rows_per_image,
        });
        self
    }

    pub fn write_texture_rgba16_float(
        &mut self,
        id: TextureId,
        data: &'a [u16],
        bytes_per_row: u32,
        rows_per_image: u32,
    ) -> &mut Self {
        self.uploads.push(Upload::TextureRgba16Float {
            id,
            data,
            bytes_per_row,
            rows_per_image,
        });
        self
    }

    pub fn write_texture_rgba16_float_region(
        &mut self,
        id: TextureId,
        data: &'a [u16],
        origin: [u32; 3],
        size: crate::Size,
        bytes_per_row: u32,
        rows_per_image: u32,
    ) -> &mut Self {
        self.uploads.push(Upload::TextureRgba16FloatRegion {
            id,
            data,
            origin,
            size,
            bytes_per_row,
            rows_per_image,
        });
        self
    }
}
