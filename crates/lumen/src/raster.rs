//! Raster frame representation for immutable Skia image outputs.

use skia_safe::{AlphaType, ColorType, Data, ImageInfo, image::CachingHint, images, surfaces};

use crate::error::RenderError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    Rgba8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlphaMode {
    Premultiplied,
    Unpremultiplied,
}

impl AlphaMode {
    pub fn to_skia(self) -> AlphaType {
        match self {
            Self::Premultiplied => AlphaType::Premul,
            Self::Unpremultiplied => AlphaType::Unpremul,
        }
    }
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
pub struct RasterFrame {
    pub image: skia_safe::Image,
    pub storage_width: u32,
    pub storage_height: u32,
    pub row_bytes: usize,
    pub pixel_format: PixelFormat,
    pub alpha_mode: AlphaMode,
    pub color_space: ColorSpaceTag,
    pub format_rect: RectI,
    pub data_rect: RectI,
}

pub type ImageFrame = RasterFrame;

impl RasterFrame {
    pub fn new(image: skia_safe::Image) -> Self {
        let storage_width = image.width().max(0) as u32;
        let storage_height = image.height().max(0) as u32;
        let format_rect = RectI::from_size(storage_width, storage_height);
        Self::with_domain(
            image,
            storage_width,
            storage_height,
            format_rect,
            format_rect,
        )
    }

    pub fn with_domain(
        image: skia_safe::Image,
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
            image,
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

    pub fn from_rgba_bytes(
        bytes: &[u8],
        storage_width: u32,
        storage_height: u32,
        row_bytes: usize,
        alpha_mode: AlphaMode,
        format_rect: RectI,
        data_rect: RectI,
    ) -> crate::Result<Self> {
        let image = make_skia_image(bytes, storage_width, storage_height, row_bytes, alpha_mode)
            .ok_or_else(|| RenderError::SurfaceAllocation {
                width: storage_width,
                height: storage_height,
            })?;
        let mut frame =
            Self::with_domain(image, storage_width, storage_height, format_rect, data_rect);
        frame.alpha_mode = alpha_mode;
        Ok(frame)
    }

    pub fn image(image: skia_safe::Image, width: u32, height: u32) -> Self {
        let rect = RectI::from_size(width, height);
        Self::with_domain(image, width, height, rect, rect)
    }

    pub fn from_image_frame(frame: ImageFrame) -> Self {
        frame
    }

    pub fn transparent(
        storage_width: u32,
        storage_height: u32,
        format_rect: RectI,
        data_rect: RectI,
        alpha_mode: AlphaMode,
    ) -> crate::Result<Self> {
        let alloc_width = storage_width.max(1);
        let alloc_height = storage_height.max(1);
        let mut surface = surfaces::raster_n32_premul((alloc_width as i32, alloc_height as i32))
            .ok_or_else(|| RenderError::SurfaceAllocation {
                width: alloc_width,
                height: alloc_height,
            })?;
        surface.canvas().clear(skia_safe::Color::TRANSPARENT);
        let image = surface.image_snapshot();
        let mut frame = Self::with_domain(image, alloc_width, alloc_height, format_rect, data_rect);
        frame.alpha_mode = alpha_mode;
        Ok(frame)
    }

    pub fn dimensions(&self) -> (u32, u32) {
        (self.format_rect.width, self.format_rect.height)
    }

    pub fn storage_dimensions(&self) -> (u32, u32) {
        (self.storage_width, self.storage_height)
    }

    pub fn format_rect(&self) -> RectI {
        self.format_rect
    }

    pub fn data_rect(&self) -> RectI {
        self.data_rect
    }

    pub fn alpha_mode(&self) -> AlphaMode {
        self.alpha_mode
    }

    pub fn color_space(&self) -> ColorSpaceTag {
        self.color_space
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

    pub fn to_skia_image(&self) -> Option<skia_safe::Image> {
        Some(self.image.clone())
    }

    pub fn image_parts(&self) -> Option<(skia_safe::Image, u32, u32)> {
        Some((self.image.clone(), self.storage_width, self.storage_height))
    }

    pub fn snapshot_image(&self) -> ImageFrame {
        self.clone()
    }

    pub fn snapshot(&self) -> crate::Result<Self> {
        Ok(self.clone())
    }

    pub fn read_pixels_into(&self, dst: &mut [u8], row_bytes: usize) -> crate::Result<()> {
        read_pixels_into_image(
            &self.image,
            self.storage_width,
            self.storage_height,
            self.alpha_mode,
            dst,
            row_bytes,
        )
    }
}

pub fn make_skia_image(
    bytes: &[u8],
    width: u32,
    height: u32,
    row_bytes: usize,
    alpha_mode: AlphaMode,
) -> Option<skia_safe::Image> {
    let expected = rgba_byte_len(width, height)?;
    if bytes.len() < expected || row_bytes < (width as usize).saturating_mul(4) {
        return None;
    }
    let info = ImageInfo::new(
        (width as i32, height as i32),
        ColorType::RGBA8888,
        alpha_mode.to_skia(),
        None,
    );
    let data = Data::new_copy(bytes);
    images::raster_from_data(&info, data, row_bytes)
}

pub fn rgba_byte_len(width: u32, height: u32) -> Option<usize> {
    let pixels = u64::from(width).checked_mul(u64::from(height))?;
    let bytes = pixels.checked_mul(4)?;
    usize::try_from(bytes).ok()
}

fn read_pixels_into_image(
    image: &skia_safe::Image,
    width: u32,
    height: u32,
    alpha_mode: AlphaMode,
    dst: &mut [u8],
    row_bytes: usize,
) -> crate::Result<()> {
    let min_row_bytes = usize::try_from(width).unwrap_or(0).saturating_mul(4);
    let required_len = row_bytes.saturating_mul(height as usize);
    if row_bytes < min_row_bytes || dst.len() < required_len {
        return Err(RenderError::PixelReadbackFailed { width, height }.into());
    }

    let info = ImageInfo::new(
        (width as i32, height as i32),
        ColorType::RGBA8888,
        alpha_mode.to_skia(),
        None,
    );
    if image.read_pixels(&info, dst, row_bytes, (0, 0), CachingHint::Disallow) {
        Ok(())
    } else {
        Err(RenderError::PixelReadbackFailed { width, height }.into())
    }
}
