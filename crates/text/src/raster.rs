use std::fmt;
use std::io::Cursor;

use cosmic_text::skrifa::{
    self, GlyphId, MetadataProvider as _,
    bitmap::{BitmapData, BitmapFormat, BitmapGlyph, BitmapStrikes, Origin as BitmapOrigin},
    instance::{LocationRef, Size},
    outline::{DrawSettings, OutlinePen},
    raw::{TableProvider as _, tables::sbix::Strike as SbixStrike, types::Tag},
};
use cosmic_text::{CacheKey, CacheKeyFlags, FontSystem, Weight, fontdb};
use zeno::{
    Angle, Bounds, Command, Format, Mask, Origin, PathData as _, Placement, Scratch, Style,
    Transform, Vector, Verb,
};

#[derive(Default)]
pub(crate) struct GlyphPath {
    pub(crate) points: Vec<Vector>,
    pub(crate) verbs: Vec<Verb>,
}

impl GlyphPath {
    pub(crate) fn is_empty(&self) -> bool {
        self.verbs.is_empty()
    }

    pub(crate) fn commands(&self) -> impl Iterator<Item = Command> + '_ {
        (&self.points[..], &self.verbs[..]).commands()
    }

    pub(crate) fn bounds(&self) -> Option<[f32; 4]> {
        let mut iter = self.points.iter();
        let first = *iter.next()?;
        let mut bounds = [first.x, first.y, first.x, first.y];
        for point in iter {
            bounds[0] = bounds[0].min(point.x);
            bounds[1] = bounds[1].min(point.y);
            bounds[2] = bounds[2].max(point.x);
            bounds[3] = bounds[3].max(point.y);
        }
        Some(bounds)
    }

    fn maybe_close(&mut self) {
        if !self.verbs.is_empty() && self.verbs.last() != Some(&Verb::Close) {
            self.verbs.push(Verb::Close);
        }
    }

    fn transform(&mut self, transform: &Transform) {
        for point in &mut self.points {
            *point = transform.transform_point(*point);
        }
    }
}

impl OutlinePen for GlyphPath {
    fn move_to(&mut self, x: f32, y: f32) {
        self.maybe_close();
        self.points.push(Vector::new(x, y));
        self.verbs.push(Verb::MoveTo);
    }

    fn line_to(&mut self, x: f32, y: f32) {
        self.points.push(Vector::new(x, y));
        self.verbs.push(Verb::LineTo);
    }

    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        self.points.push(Vector::new(cx0, cy0));
        self.points.push(Vector::new(x, y));
        self.verbs.push(Verb::QuadTo);
    }

    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        self.points.push(Vector::new(cx0, cy0));
        self.points.push(Vector::new(cx1, cy1));
        self.points.push(Vector::new(x, y));
        self.verbs.push(Verb::CurveTo);
    }

    fn close(&mut self) {
        self.verbs.push(Verb::Close);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RasterContent {
    Mask,
    Color,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RasterSource {
    Outline,
    ColorOutline,
    ColorBitmap { straight_alpha: bool },
}

pub(crate) struct RasterImage {
    pub(crate) placement: Placement,
    pub(crate) content: RasterContent,
    pub(crate) source: RasterSource,
    pub(crate) data: Vec<u8>,
}

pub(crate) enum MaskOutcome {
    Rendered(Vec<u8>, Placement),
    Empty,
    Oversized,
}

pub(crate) enum ImageOutcome {
    Rendered(RasterImage),
    Empty,
    Oversized,
}

pub(crate) struct GlyphRasterizer {
    scratch: Scratch,
}

impl fmt::Debug for GlyphRasterizer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad("GlyphRasterizer { .. }")
    }
}

impl GlyphRasterizer {
    pub(crate) fn new() -> Self {
        Self {
            scratch: Scratch::new(),
        }
    }

    /// Scales a glyph's unhinted outline using `skrifa` directly so no hinting engine is linked.
    pub(crate) fn scale_outline(
        &self,
        font_system: &mut FontSystem,
        key: CacheKey,
    ) -> Option<GlyphPath> {
        with_skrifa_font(font_system, key.font_id, key.font_weight, |font_ref| {
            scale_unhinted_outline(&font_ref, key.glyph_id, key)
        })?
    }

    /// Rasterizes `path` into an 8-bit alpha mask if both dimensions fit within `max_size`,
    /// inspecting exact styled mask dimensions before allocating the pixel buffer.
    pub(crate) fn rasterize_mask_within(
        &mut self,
        path: &GlyphPath,
        style: Style<'_>,
        offset: Vector,
        max_size: u32,
    ) -> MaskOutcome {
        let mut size = [0_u32; 2];
        let mut mask = Mask::with_scratch((&path.points[..], &path.verbs[..]), &mut self.scratch);
        mask.format(Format::Alpha)
            .origin(Origin::BottomLeft)
            .style(style)
            .offset(offset)
            .render_offset(offset)
            .inspect(|_, w, h| size = [w, h]);
        if size[0] > max_size || size[1] > max_size {
            return MaskOutcome::Oversized;
        }
        if size[0] == 0 || size[1] == 0 {
            return MaskOutcome::Empty;
        }
        let mut data = vec![0_u8; Format::Alpha.buffer_size(size[0], size[1])];
        let placement = mask.render_into(&mut data[..], None);
        MaskOutcome::Rendered(data, placement)
    }

    /// Renders a glyph from `COLR` v0 outlines, `sbix`/`CBDT` colour bitmaps, or its unhinted
    /// monochrome outline, in that priority order.
    pub(crate) fn render_image(
        &mut self,
        font_system: &mut FontSystem,
        key: CacheKey,
    ) -> Option<RasterImage> {
        match self.render_image_within(font_system, key, None) {
            ImageOutcome::Rendered(image) => Some(image),
            ImageOutcome::Empty | ImageOutcome::Oversized => None,
        }
    }

    /// Renders a glyph as [`render_image`](Self::render_image) does, rejecting glyphs whose pixel
    /// dimensions exceed `max_size` before allocating their pixel buffers.
    pub(crate) fn render_image_within(
        &mut self,
        font_system: &mut FontSystem,
        key: CacheKey,
        max_size: Option<u32>,
    ) -> ImageOutcome {
        let offset = if key.flags.contains(CacheKeyFlags::PIXEL_FONT) {
            Vector::new(key.x_bin.as_float().round(), key.y_bin.as_float().round())
        } else {
            Vector::new(key.x_bin.as_float(), key.y_bin.as_float())
        };
        with_skrifa_font(font_system, key.font_id, key.font_weight, |font_ref| {
            if let Some(outcome) = self.render_color_outline(&font_ref, key, offset, max_size) {
                return outcome;
            }
            if let Some(outcome) = render_color_bitmap(&font_ref, key, max_size) {
                return outcome;
            }
            let Some(path) = scale_unhinted_outline(&font_ref, key.glyph_id, key) else {
                return ImageOutcome::Empty;
            };
            match self.rasterize_mask_within(
                &path,
                Style::default(),
                offset,
                max_size.unwrap_or(u32::MAX),
            ) {
                MaskOutcome::Oversized => ImageOutcome::Oversized,
                MaskOutcome::Empty => ImageOutcome::Empty,
                MaskOutcome::Rendered(data, placement) => ImageOutcome::Rendered(RasterImage {
                    placement,
                    content: RasterContent::Mask,
                    source: RasterSource::Outline,
                    data,
                }),
            }
        })
        .unwrap_or(ImageOutcome::Empty)
    }

    fn render_color_outline(
        &mut self,
        font_ref: &skrifa::FontRef<'_>,
        key: CacheKey,
        offset: Vector,
        max_size: Option<u32>,
    ) -> Option<ImageOutcome> {
        let colr = font_ref.colr().ok()?;
        let cpal = font_ref.cpal().ok()?;
        let layer_range = colr.v0_base_glyph(GlyphId::from(key.glyph_id)).ok()??;
        let palette_start = cpal
            .color_record_indices()
            .first()
            .map_or(0, |index| index.get() as usize);
        let color_records = cpal.color_records_array()?.ok()?;

        let mut layers = Vec::with_capacity(layer_range.len());
        let mut all_points = Vec::new();
        for index in layer_range {
            let (layer_glyph, color_index) = colr.v0_layer(index).ok()?;
            let path = scale_unhinted_outline(font_ref, layer_glyph.to_u16(), key)?;
            all_points.extend_from_slice(&path.points);
            let color = if color_index == 0xFFFF {
                [128, 128, 128, 255]
            } else {
                color_records
                    .get(palette_start + usize::from(color_index))
                    .map_or([128, 128, 128, 255], |record| {
                        [record.red(), record.green(), record.blue(), record.alpha()]
                    })
            };
            layers.push((path, color));
        }

        if all_points.is_empty() {
            return Some(ImageOutcome::Empty);
        }
        let total_bounds = Bounds::from_points(&all_points);
        let base_x = (total_bounds.min.x + offset.x).floor() as i32;
        let base_y = (total_bounds.min.y + offset.y).ceil() as i32;
        let base_w = total_bounds.width().ceil().max(0.0) as u32;
        let base_h = total_bounds.height().ceil().max(0.0) as u32;
        if let Some(max) = max_size
            && (base_w > max || base_h > max)
        {
            return Some(ImageOutcome::Oversized);
        }
        if base_w == 0 || base_h == 0 {
            return Some(ImageOutcome::Empty);
        }
        let mut data = vec![0_u8; (base_w as usize) * (base_h as usize) * 4];
        let placement = Placement {
            left: base_x,
            top: base_h as i32 + base_y,
            width: base_w,
            height: base_h,
        };
        let layer_limit = max_size.unwrap_or(u32::MAX);
        for (path, color) in &layers {
            match self.rasterize_mask_within(path, Style::default(), offset, layer_limit) {
                MaskOutcome::Oversized => return Some(ImageOutcome::Oversized),
                MaskOutcome::Empty => continue,
                MaskOutcome::Rendered(mask, layer_placement) => {
                    blit_premultiplied(
                        &mask,
                        layer_placement.width,
                        layer_placement.height,
                        layer_placement.left.wrapping_sub(base_x),
                        (base_h as i32 + base_y).wrapping_sub(layer_placement.top),
                        *color,
                        &mut data,
                        base_w,
                        base_h,
                    );
                }
            }
        }
        Some(ImageOutcome::Rendered(RasterImage {
            placement,
            content: RasterContent::Color,
            source: RasterSource::ColorOutline,
            data,
        }))
    }
}

pub(crate) fn with_skrifa_font<R>(
    font_system: &mut FontSystem,
    font_id: fontdb::ID,
    weight: Weight,
    f: impl FnOnce(skrifa::FontRef<'_>) -> R,
) -> Option<R> {
    let face_index = font_system.db().face(font_id)?.index;
    let font = font_system.get_font(font_id, weight)?;
    let font_ref = skrifa::FontRef::from_index(font.data(), face_index).ok()?;
    Some(f(font_ref))
}

pub(crate) fn has_colour_tables(font_ref: &skrifa::FontRef<'_>) -> bool {
    [b"COLR", b"CBDT", b"sbix", b"EBDT"]
        .iter()
        .any(|tag| font_ref.table_data(Tag::new(tag)).is_some())
}

/// Resolves `sbix` `dupe` indirections in `strike` with a bounded hop count, returning the
/// glyph ID whose `sbix` record holds a `png ` bitmap.
fn resolve_sbix_png_glyph(strike: &SbixStrike<'_>, mut glyph_id: GlyphId) -> Option<GlyphId> {
    const MAX_SBIX_DUPE_HOPS: usize = 8;
    for _ in 0..=MAX_SBIX_DUPE_HOPS {
        let data = strike.glyph_data(glyph_id).ok()??;
        let graphic_type = data.graphic_type();
        if graphic_type == Tag::new(b"png ") {
            return Some(glyph_id);
        }
        if graphic_type == Tag::new(b"dupe") {
            let bytes = data.data();
            if bytes.len() < 2 {
                return None;
            }
            let target = GlyphId::from(u16::from_be_bytes([bytes[0], bytes[1]]));
            if target == glyph_id {
                return None;
            }
            glyph_id = target;
            continue;
        }
        return None;
    }
    None
}

fn sbix_glyph_for_size<'a>(
    font_ref: &skrifa::FontRef<'a>,
    size_px: f32,
    glyph_id: GlyphId,
) -> Option<BitmapGlyph<'a>> {
    let sbix = font_ref.sbix().ok()?;
    let strikes = BitmapStrikes::with_format(font_ref, BitmapFormat::Sbix)?;
    let mut best: Option<BitmapGlyph<'a>> = None;
    for index in 0..sbix.strikes().len() {
        let Ok(raw_strike) = sbix.strikes().get(index) else {
            continue;
        };
        let entry_size = f32::from(raw_strike.ppem());
        let dominated = best.as_ref().is_some_and(|current| {
            let best_size = current.ppem_y;
            !((entry_size >= size_px && entry_size < best_size)
                || (best_size < size_px && entry_size > best_size))
        });
        if dominated {
            continue;
        }
        if let Some(target) = resolve_sbix_png_glyph(&raw_strike, glyph_id)
            && let Some(strike) = strikes.get(index)
            && let Some(mut glyph) = strike.get(target)
        {
            if target != glyph_id {
                let metrics = font_ref.glyph_metrics(Size::unscaled(), LocationRef::default());
                glyph.bearing_x = metrics.left_side_bearing(glyph_id).unwrap_or_default();
                glyph.bearing_y = metrics.bounds(glyph_id).unwrap_or_default().y_min;
            }
            best = Some(glyph);
        }
    }
    best
}

pub(crate) fn has_colour_bitmap_glyph(font_ref: &skrifa::FontRef<'_>, glyph: u16) -> bool {
    let glyph_id = GlyphId::from(glyph);
    if let Ok(sbix) = font_ref.sbix()
        && sbix.strikes().iter().any(|strike| {
            strike
                .ok()
                .and_then(|strike| resolve_sbix_png_glyph(&strike, glyph_id))
                .is_some()
        })
    {
        return true;
    }
    BitmapStrikes::with_format(font_ref, BitmapFormat::Cbdt)
        .is_some_and(|strikes| strikes.iter().any(|strike| strike.get(glyph_id).is_some()))
}

fn scale_unhinted_outline(
    font_ref: &skrifa::FontRef<'_>,
    glyph_id: u16,
    key: CacheKey,
) -> Option<GlyphPath> {
    let outlines = font_ref.outline_glyphs();
    let glyph = outlines.get(GlyphId::from(glyph_id))?;
    let upem = font_ref.head().ok().map_or(0, |head| head.units_per_em());
    let size_px = f32::from_bits(key.font_size_bits).max(0.0);
    let size = if size_px != 0.0 && upem != 0 {
        Size::new(size_px)
    } else {
        Size::unscaled()
    };
    let location = font_ref
        .axes()
        .location([(Tag::new(b"wght"), f32::from(key.font_weight.0))]);
    let mut path = GlyphPath::default();
    glyph
        .draw(DrawSettings::unhinted(size, &location), &mut path)
        .ok()?;
    path.maybe_close();
    if key.flags.contains(CacheKeyFlags::FAKE_ITALIC) {
        path.transform(&Transform::skew(
            Angle::from_degrees(14.0),
            Angle::from_degrees(0.0),
        ));
    }
    Some(path)
}

fn render_color_bitmap(
    font_ref: &skrifa::FontRef<'_>,
    key: CacheKey,
    max_size: Option<u32>,
) -> Option<ImageOutcome> {
    let size_px = f32::from_bits(key.font_size_bits).max(0.0);
    if size_px == 0.0 {
        return None;
    }
    let glyph_id = GlyphId::from(key.glyph_id);
    let bitmap = sbix_glyph_for_size(font_ref, size_px, glyph_id).or_else(|| {
        BitmapStrikes::with_format(font_ref, BitmapFormat::Cbdt)?
            .glyph_for_size(Size::new(size_px), glyph_id)
    })?;
    if bitmap.ppem_y <= 0.0 {
        return None;
    }
    let scale = size_px / bitmap.ppem_y;
    let needs_resize = (scale - 1.0).abs() > 1e-4;
    let (mut data, mut width, mut height, mut straight_alpha) = match bitmap.data {
        BitmapData::Png(bytes) => {
            let decoder = image::codecs::png::PngDecoder::new(Cursor::new(bytes)).ok()?;
            let (raw_w, raw_h) = image::ImageDecoder::dimensions(&decoder);
            if raw_w == 0 || raw_h == 0 {
                return None;
            }
            let final_w = if needs_resize {
                ((raw_w as f32 * scale) as u32).max(1)
            } else {
                raw_w
            };
            let final_h = if needs_resize {
                ((raw_h as f32 * scale) as u32).max(1)
            } else {
                raw_h
            };
            if let Some(max) = max_size
                && (final_w > max || final_h > max)
            {
                return Some(ImageOutcome::Oversized);
            }
            let rgba = image::DynamicImage::from_decoder(decoder)
                .ok()?
                .into_rgba8();
            let (w, h) = rgba.dimensions();
            (rgba.into_raw(), w, h, true)
        }
        BitmapData::Bgra(bytes) => {
            if bitmap.width == 0 || bitmap.height == 0 {
                return None;
            }
            let final_w = if needs_resize {
                ((bitmap.width as f32 * scale) as u32).max(1)
            } else {
                bitmap.width
            };
            let final_h = if needs_resize {
                ((bitmap.height as f32 * scale) as u32).max(1)
            } else {
                bitmap.height
            };
            if let Some(max) = max_size
                && (final_w > max || final_h > max)
            {
                return Some(ImageOutcome::Oversized);
            }
            let expected = (bitmap.width as usize) * (bitmap.height as usize) * 4;
            if bytes.len() < expected {
                return None;
            }
            let mut rgba = Vec::with_capacity(expected);
            for &[b, g, r, a] in bytes[..expected].as_chunks::<4>().0 {
                rgba.extend_from_slice(&[r, g, b, a]);
            }
            (rgba, bitmap.width, bitmap.height, false)
        }
        BitmapData::Mask(_) => return None,
    };
    if width == 0 || height == 0 {
        return None;
    }
    let upem = font_ref.head().ok().map_or(0, |head| head.units_per_em());
    let (mut left, mut top) = match bitmap.placement_origin {
        BitmapOrigin::TopLeft => (bitmap.inner_bearing_x, bitmap.inner_bearing_y),
        BitmapOrigin::BottomLeft => {
            let scale_fu = if upem != 0 {
                bitmap.ppem_y / f32::from(upem)
            } else {
                0.0
            };
            (
                bitmap.bearing_x * scale_fu + bitmap.inner_bearing_x,
                bitmap.bearing_y * scale_fu + bitmap.inner_bearing_y + height as f32,
            )
        }
    };
    if needs_resize {
        let target_w = ((width as f32 * scale) as u32).max(1);
        let target_h = ((height as f32 * scale) as u32).max(1);
        if let Some(max) = max_size
            && (target_w > max || target_h > max)
        {
            return Some(ImageOutcome::Oversized);
        }
        // Straight-alpha PNGs must be premultiplied before spatial filtering so transparent
        // neighbours do not darken edges when the atlas stores premultiplied pixels.
        if straight_alpha {
            premultiply_rgba8(&mut data);
            straight_alpha = false;
        }
        data = resize_rgba8(&data, width, height, target_w, target_h);
        width = target_w;
        height = target_h;
        left *= scale;
        top *= scale;
    }
    Some(ImageOutcome::Rendered(RasterImage {
        placement: Placement {
            left: left as i32,
            top: top as i32,
            width,
            height,
        },
        content: RasterContent::Color,
        source: RasterSource::ColorBitmap { straight_alpha },
        data,
    }))
}

#[allow(clippy::too_many_arguments)]
fn blit_premultiplied(
    mask: &[u8],
    mask_width: u32,
    mask_height: u32,
    x: i32,
    y: i32,
    color: [u8; 4],
    target: &mut [u8],
    target_width: u32,
    target_height: u32,
) {
    if mask_width == 0 || mask_height == 0 || target_width == 0 || target_height == 0 {
        return;
    }
    let source_width = mask_width as usize;
    let source_height = mask_height as usize;
    let dest_width = target_width as usize;
    let dest_height = target_height as usize;
    let source_x = if x < 0 { (-x) as usize } else { 0 };
    let source_y = if y < 0 { (-y) as usize } else { 0 };
    if source_x >= source_width || source_y >= source_height {
        return;
    }
    let dest_x = if x < 0 { 0 } else { x as usize };
    let dest_y = if y < 0 { 0 } else { y as usize };
    if dest_x >= dest_width || dest_y >= dest_height {
        return;
    }
    let source_end_x = source_width.min(dest_width - dest_x + source_x);
    let source_end_y = source_height.min(dest_height - dest_y + source_y);
    let dest_pitch = dest_width * 4;
    let color_a = u32::from(color[3]);
    for (dy, sy) in (dest_y..).zip(source_y..source_end_y) {
        let src_row = &mask[sy * source_width..];
        let dst_row = &mut target[dy * dest_pitch..];
        let mut dx = dest_x * 4;
        for &coverage in &src_row[source_x..source_end_x] {
            let a = (u32::from(coverage) * color_a) >> 8;
            if a >= 255 {
                dst_row[dx..dx + 3].copy_from_slice(&color[..3]);
                dst_row[dx + 3] = 255;
            } else if a != 0 {
                let inverse_a = 255 - a;
                for i in 0..3 {
                    let d = u32::from(dst_row[dx + i]);
                    dst_row[dx + i] = ((inverse_a * d + a * u32::from(color[i])) >> 8) as u8;
                }
                let d = u32::from(dst_row[dx + 3]);
                dst_row[dx + 3] = ((inverse_a * d + a * 255) >> 8) as u8;
            }
            dx += 4;
        }
    }
}

fn premultiply_rgba8(rgba: &mut [u8]) {
    for px in rgba.as_chunks_mut::<4>().0 {
        let a = u16::from(px[3]);
        px[0] = ((u16::from(px[0]) * a + 127) / 255) as u8;
        px[1] = ((u16::from(px[1]) * a + 127) / 255) as u8;
        px[2] = ((u16::from(px[2]) * a + 127) / 255) as u8;
    }
}

/// Mitchell-Netravali kernel matching `swash::scale::bitmap::mitchell`.
fn mitchell(x: f32) -> f32 {
    let x = x.abs();
    if x < 1.0 {
        ((16.0 + x * x * (21.0 * x - 36.0)) / 18.0).abs()
    } else if x < 2.0 {
        ((32.0 + x * (-60.0 + x * (36.0 - 7.0 * x))) / 18.0).abs()
    } else {
        0.0
    }
}

/// Resizes an RGBA8 bitmap with a separable two-pass Mitchell filter whose support widens when
/// downscaling so intervening source pixels contribute without aliasing.
fn resize_rgba8(src: &[u8], src_w: u32, src_h: u32, dst_w: u32, dst_h: u32) -> Vec<u8> {
    let mut tmp = vec![0_u8; (dst_w as usize) * (src_h as usize) * 4];
    let mut dst = vec![0_u8; (dst_w as usize) * (dst_h as usize) * 4];
    let mut weights = Vec::new();
    resample_axis(
        src_w,
        src_h,
        dst_w,
        &mut weights,
        |x, y| {
            let idx = ((y * src_w + x) * 4) as usize;
            [src[idx], src[idx + 1], src[idx + 2], src[idx + 3]]
        },
        |x, y, px| {
            let idx = ((y * dst_w + x) * 4) as usize;
            tmp[idx..idx + 4].copy_from_slice(&px);
        },
    );
    resample_axis(
        src_h,
        dst_w,
        dst_h,
        &mut weights,
        |y, x| {
            let idx = ((y * dst_w + x) * 4) as usize;
            [tmp[idx], tmp[idx + 1], tmp[idx + 2], tmp[idx + 3]]
        },
        |y, x, px| {
            let idx = ((y * dst_w + x) * 4) as usize;
            dst[idx..idx + 4].copy_from_slice(&px);
        },
    );
    dst
}

fn resample_axis(
    src_len: u32,
    lines: u32,
    dst_len: u32,
    weights: &mut Vec<f32>,
    read: impl Fn(u32, u32) -> [u8; 4],
    mut write: impl FnMut(u32, u32, [u8; 4]),
) {
    const SUPPORT: f32 = 2.0;
    let ratio = src_len as f32 / dst_len as f32;
    let scale = ratio.max(1.0);
    let src_support = SUPPORT * scale;
    let inv_scale = 1.0 / scale;
    for out_i in 0..dst_len {
        let center = (out_i as f32 + 0.5) * ratio;
        let left = ((center - src_support).floor() as i32).clamp(0, src_len as i32 - 1) as u32;
        let right =
            ((center + src_support).ceil() as i32).clamp(left as i32 + 1, src_len as i32) as u32;
        let sample_center = center - 0.5;
        weights.clear();
        let mut sum = 0.0_f32;
        for src_i in left..right {
            let w = mitchell((src_i as f32 - sample_center) * inv_scale);
            weights.push(w);
            sum += w;
        }
        if sum > 0.0 {
            let inv_sum = 1.0 / sum;
            for w in weights.iter_mut() {
                *w *= inv_sum;
            }
        }
        for line in 0..lines {
            let mut acc = [0.0_f32; 4];
            for (offset, &w) in weights.iter().enumerate() {
                let px = read(left + offset as u32, line);
                for c in 0..4 {
                    acc[c] += f32::from(px[c]) * w;
                }
            }
            write(out_i, line, acc.map(|v| v.round().clamp(0.0, 255.0) as u8));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeno::{Join, Stroke};

    #[test]
    fn oversized_curved_stroke_is_rejected_before_allocation() {
        // Coincident on-curve endpoints with a huge off-curve control point.
        let mut path = GlyphPath::default();
        path.move_to(0.0, 0.0);
        path.quad_to(10_000.0, 10_000.0, 0.0, 0.0);
        path.close();

        let mut stroke = Stroke::new(4.0);
        stroke.join(Join::Round);
        let mut rasterizer = GlyphRasterizer::new();
        assert!(matches!(
            rasterizer.rasterize_mask_within(&path, stroke.into(), Vector::new(0.0, 0.0), 64),
            MaskOutcome::Oversized
        ));
    }

    #[test]
    fn mitchell_downscale_averages_high_frequency_stripes() {
        let (src_w, src_h) = (128_u32, 128_u32);
        let mut src = Vec::with_capacity((src_w * src_h * 4) as usize);
        for y in 0..src_h {
            for x in 0..src_w {
                let v = if (x + y) % 2 == 0 { 255_u8 } else { 0_u8 };
                src.extend_from_slice(&[v, v, v, 255]);
            }
        }
        let dst = resize_rgba8(&src, src_w, src_h, 16, 16);
        let center = ((8 * 16 + 8) * 4) as usize;
        assert!(
            (110..=145).contains(&dst[center]),
            "expected anti-aliased mid-grey around 128, got {}",
            dst[center]
        );
    }

    fn build_sbix_dupe_font(png_bytes: &[u8]) -> Vec<u8> {
        // Minimal SFNT with head, hhea, maxp (4 glyphs), hmtx, loca, glyf, and sbix:
        // glyph 0 = empty, glyph 1 = png, glyph 2 = dupe -> 1, glyph 3 = dupe -> 3 (cycle).
        let num_glyphs: u16 = 4;
        let mut head = vec![0_u8; 54];
        head[18..20].copy_from_slice(&1000_u16.to_be_bytes()); // unitsPerEm
        head[50..52].copy_from_slice(&0_u16.to_be_bytes()); // indexToLocFormat = short

        let mut hhea = vec![0_u8; 36];
        hhea[34..36].copy_from_slice(&num_glyphs.to_be_bytes());

        let mut maxp = vec![0_u8; 6];
        maxp[0..4].copy_from_slice(&0x0000_5000_u32.to_be_bytes()); // version 0.5
        maxp[4..6].copy_from_slice(&num_glyphs.to_be_bytes());

        let mut hmtx = vec![0_u8; usize::from(num_glyphs) * 4];
        // Give glyph 1 lsb = 100 and glyph 2 (dupe -> 1) lsb = 500 to verify dupe keeps its own outer bearings.
        hmtx[4 + 2..4 + 4].copy_from_slice(&100_i16.to_be_bytes());
        hmtx[8 + 2..8 + 4].copy_from_slice(&500_i16.to_be_bytes());
        let loca = vec![0_u8; (usize::from(num_glyphs) + 1) * 2];
        let glyf = vec![0_u8; 4];

        // Build one sbix strike at ppem=64.
        let rec1 = {
            let mut r = Vec::new();
            r.extend_from_slice(&0_i16.to_be_bytes());
            r.extend_from_slice(&0_i16.to_be_bytes());
            r.extend_from_slice(b"png ");
            r.extend_from_slice(png_bytes);
            r
        };
        let rec2 = {
            let mut r = Vec::new();
            r.extend_from_slice(&0_i16.to_be_bytes());
            r.extend_from_slice(&0_i16.to_be_bytes());
            r.extend_from_slice(b"dupe");
            r.extend_from_slice(&1_u16.to_be_bytes());
            r
        };
        let rec3 = {
            let mut r = Vec::new();
            r.extend_from_slice(&0_i16.to_be_bytes());
            r.extend_from_slice(&0_i16.to_be_bytes());
            r.extend_from_slice(b"dupe");
            r.extend_from_slice(&3_u16.to_be_bytes());
            r
        };
        let strike_header = 4 + (usize::from(num_glyphs) + 1) * 4;
        let off0 = strike_header as u32;
        let off1 = off0;
        let off2 = off1 + rec1.len() as u32;
        let off3 = off2 + rec2.len() as u32;
        let off4 = off3 + rec3.len() as u32;
        let mut strike = Vec::new();
        strike.extend_from_slice(&64_u16.to_be_bytes()); // ppem
        strike.extend_from_slice(&72_u16.to_be_bytes()); // ppi
        for off in [off0, off1, off2, off3, off4] {
            strike.extend_from_slice(&off.to_be_bytes());
        }
        strike.extend_from_slice(&rec1);
        strike.extend_from_slice(&rec2);
        strike.extend_from_slice(&rec3);

        let mut sbix = Vec::new();
        sbix.extend_from_slice(&1_u16.to_be_bytes()); // version
        sbix.extend_from_slice(&1_u16.to_be_bytes()); // flags
        sbix.extend_from_slice(&1_u32.to_be_bytes()); // numStrikes
        sbix.extend_from_slice(&12_u32.to_be_bytes()); // strike offset
        sbix.extend_from_slice(&strike);

        let mut tables: Vec<(&[u8; 4], Vec<u8>)> = vec![
            (b"glyf", glyf),
            (b"head", head),
            (b"hhea", hhea),
            (b"hmtx", hmtx),
            (b"loca", loca),
            (b"maxp", maxp),
            (b"sbix", sbix),
        ];
        tables.sort_by_key(|(tag, _)| *tag);

        let num_tables = tables.len() as u16;
        let mut out = Vec::new();
        out.extend_from_slice(&0x0001_0000_u32.to_be_bytes());
        out.extend_from_slice(&num_tables.to_be_bytes());
        out.extend_from_slice(&[0_u8; 6]);
        let mut offset = 12 + tables.len() * 16;
        for (tag, data) in &tables {
            out.extend_from_slice(*tag);
            out.extend_from_slice(&0_u32.to_be_bytes());
            out.extend_from_slice(&(offset as u32).to_be_bytes());
            out.extend_from_slice(&(data.len() as u32).to_be_bytes());
            offset += (data.len() + 3) & !3;
        }
        for (_, data) in tables {
            out.extend_from_slice(&data);
            while !out.len().is_multiple_of(4) {
                out.push(0);
            }
        }
        out
    }

    #[test]
    fn sbix_dupe_glyph_resolves_and_stops_on_cycles() {
        let mut png = Vec::new();
        {
            let img = image::RgbaImage::from_pixel(2, 2, image::Rgba([200, 100, 50, 255]));
            img.write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
                .unwrap();
        }
        let font_bytes = build_sbix_dupe_font(&png);
        let font_ref = skrifa::FontRef::new(&font_bytes).unwrap();

        assert!(has_colour_bitmap_glyph(&font_ref, 1));
        assert!(has_colour_bitmap_glyph(&font_ref, 2));
        assert!(!has_colour_bitmap_glyph(&font_ref, 3));

        let key = |glyph_id| CacheKey {
            font_id: fontdb::ID::dummy(),
            glyph_id,
            font_size_bits: 64.0_f32.to_bits(),
            x_bin: cosmic_text::SubpixelBin::Zero,
            y_bin: cosmic_text::SubpixelBin::Zero,
            font_weight: Weight::NORMAL,
            flags: CacheKeyFlags::DISABLE_HINTING,
        };
        let ImageOutcome::Rendered(orig) =
            render_color_bitmap(&font_ref, key(1), Some(256)).expect("glyph 1")
        else {
            panic!("expected rendered glyph 1");
        };
        let ImageOutcome::Rendered(dupe) =
            render_color_bitmap(&font_ref, key(2), Some(256)).expect("glyph 2")
        else {
            panic!("expected rendered dupe glyph 2");
        };
        assert_eq!(dupe.data, orig.data);
        assert_eq!(orig.placement.left, 6);
        assert_eq!(dupe.placement.left, 32);
        assert_eq!(
            (
                dupe.placement.top,
                dupe.placement.width,
                dupe.placement.height,
            ),
            (
                orig.placement.top,
                orig.placement.width,
                orig.placement.height,
            ),
        );
        assert!(render_color_bitmap(&font_ref, key(3), Some(256)).is_none());
    }

    #[test]
    fn png_resize_premultiplies_before_filtering_transparent_edges() {
        let mut png = Vec::new();
        {
            let mut img = image::RgbaImage::new(2, 1);
            img.put_pixel(0, 0, image::Rgba([255, 0, 0, 255]));
            img.put_pixel(1, 0, image::Rgba([0, 0, 0, 0]));
            img.write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
                .unwrap();
        }
        let font_bytes = build_sbix_dupe_font(&png);
        let font_ref = skrifa::FontRef::new(&font_bytes).unwrap();
        // Strike ppem is 64; requesting 32 scales 2x1 -> 1x1.
        let key = CacheKey {
            font_id: fontdb::ID::dummy(),
            glyph_id: 1,
            font_size_bits: 32.0_f32.to_bits(),
            x_bin: cosmic_text::SubpixelBin::Zero,
            y_bin: cosmic_text::SubpixelBin::Zero,
            font_weight: Weight::NORMAL,
            flags: CacheKeyFlags::DISABLE_HINTING,
        };
        let ImageOutcome::Rendered(img) =
            render_color_bitmap(&font_ref, key, Some(256)).expect("resized glyph")
        else {
            panic!("expected rendered resized glyph");
        };
        assert_eq!(
            img.source,
            RasterSource::ColorBitmap {
                straight_alpha: false
            }
        );
        assert_eq!(img.data, vec![128, 0, 0, 128]);
    }
}
