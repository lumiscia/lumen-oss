#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Size {
    pub width: u32,
    pub height: u32,
}

impl Size {
    pub const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }

    pub(crate) fn as_extent(self) -> wgpu::Extent3d {
        wgpu::Extent3d {
            width: self.width.max(1),
            height: self.height.max(1),
            depth_or_array_layers: 1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RectI {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl RectI {
    pub const fn new(x: i32, y: i32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub const fn from_size(size: Size) -> Self {
        Self::new(0, 0, size.width, size.height)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextureDomain {
    pub storage_size: Size,
    pub display_rect: RectI,
    pub data_rect: RectI,
}

impl TextureDomain {
    pub const fn full_frame(size: Size) -> Self {
        let rect = RectI::from_size(size);
        Self {
            storage_size: size,
            display_rect: rect,
            data_rect: rect,
        }
    }
}
