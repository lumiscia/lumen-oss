//! Raster frame representation for bitmap and pooled-surface backed data.

use std::sync::Arc;

use crate::{
    error::LumenError,
    node::pixel_utils::{into_bitmap_parts, read_surface_rgba, rgba_byte_len},
    surface_pool::{SurfacePool, SurfaceRef},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    Rgba8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlphaMode {
    Premultiplied,
    Unpremultiplied,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorSpaceTag {
    Srgb,
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

    pub const fn from_size(width: u32, height: u32) -> Self {
        Self::new(0, 0, width, height)
    }

    pub fn right(&self) -> i64 {
        i64::from(self.x) + i64::from(self.width)
    }

    pub fn bottom(&self) -> i64 {
        i64::from(self.y) + i64::from(self.height)
    }

    pub fn contains(&self, other: &Self) -> bool {
        i64::from(other.x) >= i64::from(self.x)
            && i64::from(other.y) >= i64::from(self.y)
            && other.right() <= self.right()
            && other.bottom() <= self.bottom()
    }

    pub fn intersect(&self, other: &Self) -> Option<Self> {
        let x0 = i64::from(self.x).max(i64::from(other.x));
        let y0 = i64::from(self.y).max(i64::from(other.y));
        let x1 = self.right().min(other.right());
        let y1 = self.bottom().min(other.bottom());
        if x1 <= x0 || y1 <= y0 {
            return None;
        }
        let width = u32::try_from(x1 - x0).ok()?;
        let height = u32::try_from(y1 - y0).ok()?;
        let x = i32::try_from(x0).ok()?;
        let y = i32::try_from(y0).ok()?;
        Some(Self::new(x, y, width, height))
    }
}

#[derive(Debug, Clone)]
pub struct BitmapFrame {
    pub pixels: Arc<Vec<u8>>,
    pub storage_width: u32,
    pub storage_height: u32,
    pub row_bytes: usize,
    pub pixel_format: PixelFormat,
    pub alpha_mode: AlphaMode,
    pub color_space: ColorSpaceTag,
    pub format_rect: RectI,
    pub data_rect: RectI,
}

impl BitmapFrame {
    pub fn new(pixels: Arc<Vec<u8>>, storage_width: u32, storage_height: u32) -> Self {
        let format_rect = RectI::from_size(storage_width, storage_height);
        Self::with_domain(
            pixels,
            storage_width,
            storage_height,
            format_rect,
            format_rect,
        )
    }

    pub fn with_domain(
        pixels: Arc<Vec<u8>>,
        storage_width: u32,
        storage_height: u32,
        format_rect: RectI,
        data_rect: RectI,
    ) -> Self {
        let row_bytes = usize::try_from(storage_width)
            .ok()
            .and_then(|width| width.checked_mul(4))
            .unwrap_or(0);
        let mut frame = Self {
            pixels,
            storage_width,
            storage_height,
            row_bytes,
            pixel_format: PixelFormat::Rgba8,
            alpha_mode: AlphaMode::Premultiplied,
            color_space: ColorSpaceTag::Srgb,
            format_rect,
            data_rect,
        };
        frame.sanitize_domain();
        frame
    }

    pub fn dimensions(&self) -> (u32, u32) {
        (self.format_rect.width, self.format_rect.height)
    }

    pub fn with_alpha_mode(mut self, alpha_mode: AlphaMode) -> Self {
        self.alpha_mode = alpha_mode;
        self
    }

    pub fn sanitize_domain(&mut self) {
        if self.row_bytes
            < usize::try_from(self.storage_width)
                .unwrap_or(0)
                .saturating_mul(4)
        {
            self.row_bytes = usize::try_from(self.storage_width)
                .ok()
                .and_then(|width| width.checked_mul(4))
                .unwrap_or(self.row_bytes);
        }
        if !self.format_rect.contains(&self.data_rect) {
            self.data_rect = self
                .format_rect
                .intersect(&self.data_rect)
                .unwrap_or(RectI::new(self.format_rect.x, self.format_rect.y, 0, 0));
        }
    }
}

#[derive(Debug)]
pub struct SurfaceFrame {
    pub surface: SurfaceRef,
    pub format_rect: RectI,
    pub data_rect: RectI,
    pub alpha_mode: AlphaMode,
    pub color_space: ColorSpaceTag,
}

impl SurfaceFrame {
    pub fn new(surface: SurfaceRef) -> Self {
        let format_rect = RectI::from_size(surface.width(), surface.height());
        Self {
            surface,
            format_rect,
            data_rect: format_rect,
            alpha_mode: AlphaMode::Premultiplied,
            color_space: ColorSpaceTag::Srgb,
        }
    }

    pub fn dimensions(&self) -> (u32, u32) {
        (self.format_rect.width, self.format_rect.height)
    }
}

#[derive(Debug)]
pub enum RasterFrame {
    Bitmap(BitmapFrame),
    Surface(SurfaceFrame),
}

impl Clone for RasterFrame {
    fn clone(&self) -> Self {
        match self {
            Self::Bitmap(frame) => Self::Bitmap(frame.clone()),
            Self::Surface(surface_frame) => {
                let width = surface_frame.surface.width();
                let height = surface_frame.surface.height();
                let bytes = surface_frame
                    .surface
                    .surface()
                    .and_then(|surface| {
                        let mut surface = surface.clone();
                        let snapshot = surface.image_snapshot();
                        snapshot
                            .peek_pixels()
                            .and_then(|pixels| pixels.bytes().map(std::borrow::ToOwned::to_owned))
                    })
                    .unwrap_or_else(|| {
                        let byte_len = rgba_byte_len(width, height).unwrap_or(4);
                        vec![0; byte_len]
                    });
                let mut bitmap = BitmapFrame::new(Arc::new(bytes), width, height)
                    .with_alpha_mode(surface_frame.alpha_mode);
                bitmap.color_space = surface_frame.color_space;
                bitmap.format_rect = surface_frame.format_rect;
                bitmap.data_rect = surface_frame.data_rect;
                bitmap.sanitize_domain();
                Self::Bitmap(bitmap)
            }
        }
    }
}

impl RasterFrame {
    pub fn bitmap(pixels: Arc<Vec<u8>>, width: u32, height: u32) -> Self {
        Self::Bitmap(BitmapFrame::new(pixels, width, height))
    }

    pub fn dimensions(&self) -> (u32, u32) {
        match self {
            Self::Bitmap(frame) => frame.dimensions(),
            Self::Surface(surface) => surface.dimensions(),
        }
    }

    pub fn format_rect(&self) -> RectI {
        match self {
            Self::Bitmap(frame) => frame.format_rect,
            Self::Surface(frame) => frame.format_rect,
        }
    }

    pub fn data_rect(&self) -> RectI {
        match self {
            Self::Bitmap(frame) => frame.data_rect,
            Self::Surface(frame) => frame.data_rect,
        }
    }

    pub fn alpha_mode(&self) -> AlphaMode {
        match self {
            Self::Bitmap(frame) => frame.alpha_mode,
            Self::Surface(frame) => frame.alpha_mode,
        }
    }

    pub fn as_bitmap_frame(&self) -> Option<&BitmapFrame> {
        match self {
            Self::Bitmap(frame) => Some(frame),
            Self::Surface(_) => None,
        }
    }

    pub fn as_bitmap_bytes(&self) -> Option<&[u8]> {
        match self {
            Self::Bitmap(frame) => Some(frame.pixels.as_slice()),
            Self::Surface(_) => None,
        }
    }

    pub fn to_bitmap(self) -> Result<Self, LumenError> {
        match self {
            Self::Bitmap(..) => Ok(self),
            Self::Surface(mut surface_frame) => {
                let width = surface_frame.surface.width();
                let height = surface_frame.surface.height();
                let bytes = match surface_frame.surface.surface_mut() {
                    Some(surface) => read_surface_rgba(surface, width, height, None),
                    None => {
                        let byte_len = rgba_byte_len(width, height).unwrap_or(4);
                        vec![0; byte_len]
                    }
                };
                let mut bitmap = BitmapFrame::new(Arc::new(bytes), width, height)
                    .with_alpha_mode(surface_frame.alpha_mode);
                bitmap.format_rect = surface_frame.format_rect;
                bitmap.data_rect = surface_frame.data_rect;
                bitmap.sanitize_domain();
                Ok(Self::Bitmap(bitmap))
            }
        }
    }

    pub fn into_parts(self) -> (Arc<Vec<u8>>, u32, u32) {
        into_bitmap_parts(self)
    }

    pub fn into_bitmap_frame(self) -> Result<BitmapFrame, LumenError> {
        match self.to_bitmap()? {
            Self::Bitmap(frame) => Ok(frame),
            Self::Surface(_) => unreachable!(),
        }
    }

    pub fn promote_to_surface(self, pool: &Arc<SurfacePool>) -> Result<Self, LumenError> {
        match self {
            Self::Surface(..) => Ok(self),
            Self::Bitmap(frame) => {
                let surface_ref = pool.acquire(frame.storage_width, frame.storage_height)?;
                let mut surface_frame = SurfaceFrame::new(surface_ref);
                surface_frame.format_rect = frame.format_rect;
                surface_frame.data_rect = frame.data_rect;
                surface_frame.alpha_mode = frame.alpha_mode;
                surface_frame.color_space = frame.color_space;
                Ok(Self::Surface(surface_frame))
            }
        }
    }
}
