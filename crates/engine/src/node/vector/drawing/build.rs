//! Turns a drawing's record into the buffers `render_2d` draws from. Each path outline is
//! flattened and banded once for all of its drawables, which read one grid for the fills and
//! strokes and another for the shadows.

use std::hash::BuildHasher;
use std::ops::Range;
use std::sync::Arc;

use bytemuck::{Pod, Zeroable};
use foldhash::{HashMap, HashSet};

use super::super::dash::GpuDash;
use super::super::drawable::{DrawableTransform, GpuDrawableTransform, MIN_SCALE, union_bounds};
use super::super::paint::GpuPaint;
use super::collect::{Geometry, MAX_DRAWABLES, Record, Source, Style, Use};
use super::path::{
    MAX_PATH_POINTS, PathPoint, STYLE_FILL_ENABLED, STYLE_STROKE_ENABLED, flattening_tolerance,
    hide_cancelled_edges, parse_path_points, point_bounds, set_dash_phases, style_flags,
};
use super::path_bands::{Drawn, GridReach, PathBins, PathGrid, band_capacity};
use super::text::{TextCache, TextGlyphs};
use super::{Path2d, Render2d, ShapeGeometryKind, Text2d};
use crate::composition::SceneId;
use crate::error::RenderError;
use crate::gpu::frame_graph::Timed;
use crate::node::NodeId;

pub(super) const DRAWING_SHADER: &str = concat!(
    include_str!("../../../gpu/shaders/alpha.wgsl"),
    include_str!("../../../gpu/shaders/color.wgsl"),
    include_str!("../paint/paint.wgsl"),
    include_str!("../drawable.wgsl"),
    include_str!("vector_common.wgsl"),
    include_str!("path_geometry.wgsl"),
    include_str!("shape_geometry.wgsl"),
    include_str!("drawing.wgsl")
);

// Values of `GpuVectorDrawable::kind`; keep in sync with `drawing.wgsl`.
const DRAWABLE_PATH: u32 = 0;
const DRAWABLE_SHAPE: u32 = 1;
// Bits of `GpuVectorDrawable::flags` beside the style bits; keep in sync with `drawing.wgsl`.
pub(super) const STYLE_SHADOW: u32 = 8;
const STYLE_OFFSET_STROKE: u32 = 16;
const DRAWABLE_SHEARED: u32 = 32;

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable, lumen_macros::ShaderLayout)]
#[shader_layout(wgsl = "VectorDrawable", source = DRAWING_SHADER)]
pub(super) struct GpuVectorDrawable {
    /// Columns of the map from the untransformed points the shader measures to those the paint
    /// is laid out in.
    pub(super) paint_frame: [[f32; 2]; 2],
    /// The box a paint in bounding-box units spans, where the paint is laid out.
    pub(super) paint_min: [f32; 2],
    pub(super) paint_size: [f32; 2],
    pub(super) position: [f32; 2],
    /// Width and height of a shape.
    pub(super) size: [f32; 2],
    pub(super) grid_origin: [f32; 2],
    pub(super) grid_scale: [f32; 2],
    pub(super) grid_size: [u32; 2],
    pub(super) kind: u32,
    pub(super) flags: u32,
    pub(super) stroke_width: f32,
    /// Untransformed distance the outline grows by.
    pub(super) fill_offset: f32,
    pub(super) fill_feather: f32,
    pub(super) trim_start: f32,
    pub(super) trim_end: f32,
    pub(super) band_origin: f32,
    pub(super) band_scale: f32,
    pub(super) band_count: u32,
    pub(super) reach: f32,
    /// Index in the bands buffer of the grid's first `(start, count)` pair.
    pub(super) grid_start: u32,
    /// Index in the bands buffer of the first band's `(start, count)` pair.
    pub(super) band_start: u32,
    pub(super) geometry_kind: u32,
    pub(super) border_radius: f32,
    /// Index in the paints buffer.
    pub(super) paint: u32,
    /// Canvas width a shadow's edge fades over, and for a shape how far it grows first.
    pub(super) shadow_softness: f32,
    pub(super) shadow_spread: f32,
    pub(super) dash_count: u32,
    pub(super) dash_period: f32,
    pub(super) dash_start_phase: f32,
    /// Index in the paints buffer of a shadow's colour, which `paint`'s alpha scales.
    pub(super) shadow_paint: u32,
    pub(super) dash: [[f32; 4]; super::super::dash::GPU_DASH_LENGTHS / 4],
}

/// One place a drawable is drawn at.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable, lumen_macros::ShaderLayout)]
#[shader_layout(wgsl = "VectorCopy", source = DRAWING_SHADER)]
pub(super) struct GpuVectorCopy {
    pub(super) transform: GpuDrawableTransform,
    /// Canvas-space `[min_x, min_y, max_x, max_y]` outside which nothing is drawn.
    pub(super) screen_bounds: [f32; 4],
    /// Linear colour multiplying the paint of a fill or stroke.
    pub(super) tint: [f32; 3],
    /// Index in the drawables buffer.
    pub(super) drawable: u32,
}

/// The copies of a drawing, lowest first, and the drawables and buffers they index.
#[derive(Default)]
pub(super) struct Drawing {
    /// What the copies of each drawable share: its outline, style and paint.
    pub(super) drawables: Vec<GpuVectorDrawable>,
    pub(super) copies: Vec<GpuVectorCopy>,
    /// Every path's points, each contour's start index counted from the first path's.
    pub(super) points: Vec<PathPoint>,
    /// Every path's bands and grids, each index into them or into `points` counted from the
    /// start of the buffer.
    pub(super) bands: Vec<u32>,
    /// The record's paints, at the indices its drawables hold, then the colours of shadows.
    pub(super) paints: Vec<GpuPaint>,
    paint_slots: HashMap<u64, u32>,
    hasher: foldhash::fast::RandomState,
}

/// The transform that applies `child`, then `parent`.
pub(super) fn then(
    child: GpuDrawableTransform,
    parent: GpuDrawableTransform,
) -> GpuDrawableTransform {
    let multiply = |a: [[f32; 2]; 2], b: [[f32; 2]; 2]| [apply(a, b[0]), apply(a, b[1])];
    let add = |a: [f32; 2], b: [f32; 2]| [a[0] + b[0], a[1] + b[1]];
    GpuDrawableTransform {
        linear: multiply(parent.linear, child.linear),
        inverse_linear: multiply(child.inverse_linear, parent.inverse_linear),
        offset: add(apply(parent.linear, child.offset), parent.offset),
        inverse_offset: add(
            apply(child.inverse_linear, parent.inverse_offset),
            child.inverse_offset,
        ),
        opacity: child.opacity * parent.opacity,
        flags: child.flags | parent.flags,
        _pad: [0; 2],
    }
}

fn apply(m: [[f32; 2]; 2], v: [f32; 2]) -> [f32; 2] {
    [
        m[0][0] * v[0] + m[1][0] * v[1],
        m[0][1] * v[0] + m[1][1] * v[1],
    ]
}

pub(super) fn identity() -> GpuDrawableTransform {
    DrawableTransform {
        position: (0.0, 0.0),
        scale: (1.0, 1.0),
        rotation: 0.0,
        anchor: (0.0, 0.0),
        opacity: 1.0,
    }
    .to_gpu()
}

/// The canvas pixels per untransformed pixel along the narrowest and widest directions of
/// `transform`.
pub(super) fn scales(transform: &GpuDrawableTransform) -> [f64; 2] {
    let [a, b] = transform.linear;
    let length = |v: [f32; 2]| v[0].hypot(v[1]);
    if length(a) == length(b) && a[0] * b[0] + a[1] * b[1] == 0.0 {
        return [f64::from(length(a)); 2];
    }
    let [a, b] = [a, b].map(|v| v.map(f64::from));
    let (e, g, f) = (
        a[0] * a[0] + a[1] * a[1],
        b[0] * b[0] + b[1] * b[1],
        a[0] * b[0] + a[1] * b[1],
    );
    let spread = ((e - g) * (e - g) + 4.0 * f * f).sqrt();
    [
        ((e + g - spread) * 0.5).max(0.0).sqrt(),
        ((e + g + spread) * 0.5).sqrt(),
    ]
}

/// Columns of the rotation by `angle` radians.
fn rotation(angle: f64) -> [[f64; 2]; 2] {
    let (sin, cos) = angle.sin_cos();
    [[cos, sin], [-sin, cos]]
}

fn apply64(m: [[f64; 2]; 2], v: [f64; 2]) -> [f64; 2] {
    [
        m[0][0] * v[0] + m[1][0] * v[1],
        m[0][1] * v[0] + m[1][1] * v[1],
    ]
}

/// Where an outline's drawables measure it from.
#[derive(Clone, Copy)]
struct Measured {
    /// The transform the shaders measure with, a rotation after a scale.
    transform: GpuDrawableTransform,
    /// Columns of the turn applied to the outline's untransformed points first, so that the
    /// rest of the transform has no shear.
    turn: Option<[[f64; 2]; 2]>,
}

impl Measured {
    /// `transform`, whose linear part is this placement's, measured as this one is.
    fn moved(&self, transform: GpuDrawableTransform) -> Measured {
        let Some(turn) = self.turn else {
            return Measured {
                transform,
                turn: None,
            };
        };
        let inverse_offset = apply64(turn, transform.inverse_offset.map(f64::from));
        Measured {
            transform: GpuDrawableTransform {
                linear: self.transform.linear,
                inverse_linear: self.transform.inverse_linear,
                inverse_offset: inverse_offset.map(|value| value as f32),
                ..transform
            },
            turn: Some(turn),
        }
    }
}

/// `transform` as a rotation after a scale, after a turn of the untransformed points when it
/// shears. The shaders measure strokes and soft edges by scaling untransformed points along
/// their axes, which keeps canvas distances only when the columns of the linear map are
/// perpendicular, as a merge with non-uniform scale around rotated content breaks.
fn measured(transform: GpuDrawableTransform) -> Measured {
    let [column_x, column_y] = transform.linear.map(|column| column.map(f64::from));
    let dot = column_x[0] * column_y[0] + column_x[1] * column_y[1];
    let lengths = column_x[0].hypot(column_x[1]) * column_y[0].hypot(column_y[1]);
    if dot.abs() <= 1e-6 * lengths {
        return Measured {
            transform,
            turn: None,
        };
    }
    // The singular value decomposition `rotation(after) * diag(sx, sy) * rotation(turn)`.
    let ([a, c], [b, d]) = (column_x, column_y);
    let (e, f, g, h) = ((a + d) * 0.5, (a - d) * 0.5, (c + b) * 0.5, (c - b) * 0.5);
    let (q, r) = (e.hypot(h), f.hypot(g));
    let (sx, sy) = (q + r, q - r);
    let (a1, a2) = (g.atan2(f), h.atan2(e));
    let after = rotation((a2 + a1) * 0.5);
    let turn = rotation((a2 - a1) * 0.5);
    let linear = [after[0].map(|v| v * sx), after[1].map(|v| v * sy)];
    // The new map's inverse is the old inverse followed by the turn: canvas to turned points.
    let inverse = transform.inverse_linear.map(|column| column.map(f64::from));
    let inverse_linear = [apply64(turn, inverse[0]), apply64(turn, inverse[1])];
    let to_f32 = |m: [[f64; 2]; 2]| m.map(|column| column.map(|value| value as f32));
    let decomposed = Measured {
        transform: GpuDrawableTransform {
            linear: to_f32(linear),
            inverse_linear: to_f32(inverse_linear),
            ..transform
        },
        turn: Some(turn),
    };
    decomposed.moved(transform)
}

/// What a path drawable is binned for and drawn with, in untransformed pixels.
#[derive(Clone, Copy, PartialEq)]
struct PathNeed {
    flags: u32,
    stroke_width: f32,
    fill_offset: f32,
    fill_feather: f32,
    /// The signed distances from the outline across which the coverage of a fill or of an
    /// offset outline's stroke varies, as both are measured from them.
    fill_span: Option<[f32; 2]>,
    /// How far past the outline it draws, beside a pixel of antialiasing.
    extent: f32,
    /// Canvas width a shadow's edge fades over.
    shadow_softness: f32,
}

/// What `drawable` of an outline grown by `offset` needs, drawn at `scales`. `None` when it
/// draws nothing.
fn path_need(drawable: &Use, offset: f32, scales: [f64; 2]) -> Option<PathNeed> {
    // Untransformed pixels per canvas pixel, for shadows measured in canvas pixels.
    let per_canvas = 1.0 / (scales[0] * scales[1]).sqrt().max(1e-12);
    let (spread, softness, shadow) = match drawable.shadow {
        Some(cast) => (
            cast.spread * per_canvas,
            cast.softness() * per_canvas,
            STYLE_SHADOW,
        ),
        None => (0.0, 0.0, 0),
    };
    let shadow_softness = drawable.shadow.map_or(0.0, |cast| cast.softness() as f32);
    match drawable.style {
        Style::Fill(fill) => {
            let fill_offset = (f64::from(offset) + spread) as f32;
            let fill_feather = (f64::from(fill.feather) + softness) as f32;
            let half = fill_feather * 0.5;
            Some(PathNeed {
                flags: style_flags(true, false, fill.edge_antialias) | shadow,
                stroke_width: 0.0,
                fill_offset,
                fill_feather,
                fill_span: Some([fill_offset - half, fill_offset + half]),
                extent: fill_offset + half,
                shadow_softness,
            })
        }
        Style::Stroke(stroke) => {
            if stroke.width <= 0.0 || stroke.trim.0 >= stroke.trim.1 {
                return None;
            }
            // A shadow's stroke reaches to the far side of the shadow's fade, from which it
            // fades in.
            let width = (f64::from(stroke.width) + 2.0 * spread + softness).max(0.0) as f32;
            let half = width * 0.5;
            let offset_stroke = offset != 0.0;
            let offset_flag = if offset_stroke {
                STYLE_OFFSET_STROKE
            } else {
                0
            };
            Some(PathNeed {
                flags: style_flags(false, true, true) | shadow | offset_flag,
                stroke_width: width,
                fill_offset: offset,
                fill_feather: 0.0,
                fill_span: offset_stroke.then_some([offset - half, offset + half]),
                extent: if offset_stroke { offset + half } else { half },
                shadow_softness,
            })
        }
    }
}

/// What a grid shared by `needs`, laid out from `position`, is binned for: a fill whose coverage
/// varies across every need's span, and a stroke as wide as the widest.
fn path_grid<'a>(needs: impl Iterator<Item = &'a PathNeed>, position: [f32; 2]) -> PathGrid {
    let mut uniforms = PathGrid {
        position,
        ..PathGrid::default()
    };
    let mut span: Option<[f32; 2]> = None;
    for need in needs {
        if let Some([low, high]) = need.fill_span {
            span = Some(span.map_or([low, high], |[a, b]| [a.min(low), b.max(high)]));
        } else if need.flags & STYLE_STROKE_ENABLED != 0 {
            uniforms.flags |= STYLE_STROKE_ENABLED;
            uniforms.stroke_width = uniforms.stroke_width.max(need.stroke_width);
        }
    }
    if let Some([low, high]) = span {
        uniforms.flags |= STYLE_FILL_ENABLED;
        uniforms.fill_offset = (low + high) * 0.5;
        uniforms.fill_feather = high - low;
    }
    uniforms
}

/// The fields drawables read a grid by.
#[derive(Clone, Copy)]
struct GridFields {
    band_origin: f32,
    band_scale: f32,
    band_count: u32,
    grid_origin: [f32; 2],
    grid_scale: [f32; 2],
    grid_size: [u32; 2],
    reach: f32,
}

/// A grid binned over a path's bands: its fields, its pair count and its buffer.
struct BuiltGrid {
    fields: GridFields,
    pairs: usize,
    data: Vec<u32>,
}

/// A path outline flattened and binned for every drawable of one placement. Its indices count
/// from the starts of its own buffers.
struct BuiltPath {
    points: Vec<PathPoint>,
    /// The bands' pairs, then their lists.
    bands: Vec<u32>,
    band_pairs: usize,
    /// The grid the fills and strokes read, then the one the shadows read.
    grids: [Option<BuiltGrid>; 2],
    /// Where the points, turned when the placement shears, are laid out from.
    position: [f32; 2],
    /// The box around the points.
    bounds: ([f32; 2], [f32; 2]),
    /// The untransformed box a paint in bounding-box units spans.
    paint_box: ([f32; 2], [f32; 2]),
}

/// What a path was built from beside its data, which a kept build must match.
#[derive(PartialEq)]
struct PathKey {
    /// The id of the text a text outline was shaped as, or 0.
    text: u64,
    transforms: Vec<GpuDrawableTransform>,
    casts: Vec<GpuDrawableTransform>,
    needs: Vec<PathNeed>,
    dash: Option<(u64, u64)>,
    canvas: [f32; 2],
}

struct KeptPath {
    key: PathKey,
    /// One path of a `path_2d`, or each path a text is drawn as.
    built: Vec<BuiltPath>,
    last_used: u64,
}

/// Paths `render_2d` built on earlier frames from path and text nodes whose params don't read
/// the frame, kept while later frames draw them the same way, and the texts recent frames shaped.
#[derive(Default)]
pub(crate) struct DrawingCache {
    paths: HashMap<(SceneId, NodeId, NodeId, usize), KeptPath>,
    pub(crate) texts: TextCache,
}

impl DrawingCache {
    /// Drops the paths and texts that neither this frame nor the last one drew.
    pub(crate) fn begin_frame(&mut self, tick: u64) {
        self.paths
            .retain(|_, kept| kept.last_used.saturating_add(1) >= tick);
        self.texts.begin_frame(tick);
    }

    pub(crate) fn clear(&mut self) {
        self.paths.clear();
        self.texts.clear();
    }
}

/// Where in the cache a `render_2d` keeps its paths, and the frame it records.
pub(crate) struct Keeping<'a> {
    pub(crate) cache: &'a mut DrawingCache,
    pub(crate) scene: SceneId,
    pub(crate) render: NodeId,
    pub(crate) tick: u64,
}

/// The drawables of one outline under placements that differ only by where they move it, and
/// the dash its points are measured for. One build of the outline serves them all.
struct Instance {
    geometry: usize,
    /// The first placement.
    measured: Measured,
    /// Every placement the shaders measure with, each once, at full opacity.
    transforms: Vec<GpuDrawableTransform>,
    scales: [f64; 2],
    dash: Option<GpuDash>,
    uses: Vec<usize>,
}

/// A built path's place in the drawing's buffers.
struct PlacedPath {
    band_start: u32,
    grids: [Option<(GridFields, u32)>; 2],
    position: [f32; 2],
    bounds: ([f32; 2], [f32; 2]),
    paint_box: ([f32; 2], [f32; 2]),
    turn: Option<[[f64; 2]; 2]>,
}

/// The bits of a transform's linear part, which drawables sharing a build of their outline share.
fn linear_key(transform: &GpuDrawableTransform) -> [u32; 8] {
    let mut key = [0; 8];
    let values = transform
        .linear
        .iter()
        .chain(&transform.inverse_linear)
        .flatten();
    for (slot, value) in key.iter_mut().zip(values) {
        *slot = value.to_bits();
    }
    key
}

/// `transform` at full opacity, as builds of outlines are keyed by it.
fn opaque(transform: GpuDrawableTransform) -> GpuDrawableTransform {
    GpuDrawableTransform {
        opacity: 1.0,
        ..transform
    }
}

impl Drawing {
    fn paint(&mut self, paint: &GpuPaint) -> u32 {
        let bytes = bytemuck::bytes_of(paint);
        let hash = self.hasher.hash_one(bytes);
        if let Some(&slot) = self.paint_slots.get(&hash)
            && bytemuck::bytes_of(&self.paints[slot as usize]) == bytes
        {
            return slot;
        }
        let slot = self.paints.len() as u32;
        self.paints.push(*paint);
        self.paint_slots.insert(hash, slot);
        slot
    }

    /// Canvas-space `[min_x, min_y, max_x, max_y]` around everything drawn.
    pub(super) fn bounds(&self) -> [f32; 4] {
        self.copies
            .iter()
            .map(|copy| copy.screen_bounds)
            .reduce(union_bounds)
            .unwrap_or_default()
    }

    /// Appends `path`'s buffers, with their indices counted from the starts of the drawing's.
    fn place(&mut self, path: &BuiltPath, turn: Option<[[f64; 2]; 2]>) -> PlacedPath {
        let point_base = self.points.len() as u32;
        self.points
            .extend(path.points.iter().map(|point| PathPoint {
                contour_start: point.contour_start + point_base,
                ..*point
            }));
        // A block's pairs start with an index into the block; the rest are point indices.
        let mut append = |block: &[u32], pairs: usize| {
            let base = self.bands.len() as u32;
            self.bands
                .extend(block.iter().enumerate().map(|(index, &value)| {
                    if index >= pairs {
                        value + point_base
                    } else if index % 2 == 0 {
                        value + base
                    } else {
                        value
                    }
                }));
            base
        };
        let band_start = append(&path.bands, path.band_pairs);
        let grids = [0, 1].map(|slot| {
            let grid = path.grids[slot].as_ref()?;
            Some((grid.fields, append(&grid.data, grid.pairs)))
        });
        PlacedPath {
            band_start,
            grids,
            position: path.position,
            bounds: path.bounds,
            paint_box: path.paint_box,
            turn,
        }
    }

    /// Adds what the copies of `drawable` share, measured from an outline grown by `offset` with
    /// the outline `path` placed. `None` when it draws nothing.
    fn push_path(
        &mut self,
        drawable: &Use,
        offset: f32,
        instance: &Instance,
        path: &PlacedPath,
    ) -> Option<Shared> {
        let need = path_need(drawable, offset, instance.scales)?;
        let shadow = need.flags & STYLE_SHADOW != 0;
        let (grid, grid_start) = path.grids[usize::from(shadow)]?;
        let extent = need.extent + (1.0 / instance.scales[0].max(MIN_SCALE)) as f32;
        let [x, y] = path.position;
        let (min, max) = path.bounds;
        let low = [min[0] - extent + x, min[1] - extent + y];
        let high = [max[0] + extent + x, max[1] + extent + y];
        if low[0] > high[0] || low[1] > high[1] {
            return None;
        }
        let (trim_start, trim_end, dash) = match drawable.style {
            Style::Stroke(stroke) => (stroke.trim.0, stroke.trim.1, stroke.dash),
            Style::Fill(_) => (0.0, 1.0, GpuDash::SOLID),
        };
        let paint_frame = path.turn.map_or([[1.0, 0.0], [0.0, 1.0]], |turn| {
            // The inverse of a turn is its transpose.
            [[turn[0][0], turn[1][0]], [turn[0][1], turn[1][1]]]
                .map(|column| column.map(|value| value as f32))
        });
        let shadow_paint = self.shadow_paint(drawable);
        self.drawables.push(GpuVectorDrawable {
            paint_frame,
            paint_min: path.paint_box.0,
            paint_size: path.paint_box.1,
            position: path.position,
            grid_origin: grid.grid_origin,
            grid_scale: grid.grid_scale,
            grid_size: grid.grid_size,
            kind: DRAWABLE_PATH,
            flags: need.flags,
            stroke_width: need.stroke_width,
            fill_offset: need.fill_offset,
            fill_feather: need.fill_feather,
            trim_start,
            trim_end,
            band_origin: grid.band_origin,
            band_scale: grid.band_scale,
            band_count: grid.band_count,
            reach: grid.reach,
            grid_start,
            band_start: path.band_start,
            paint: drawable.paint,
            shadow_softness: need.shadow_softness,
            dash_count: dash.count,
            dash_period: dash.period(),
            dash_start_phase: dash.phase(0.0),
            dash: dash.lengths,
            shadow_paint,
            ..Zeroable::zeroed()
        });
        Some(Shared {
            drawable: self.drawables.len() as u32 - 1,
            low,
            high,
            margin: 0.0,
        })
    }

    /// Adds what the copies of `drawable` of shape `geometry` share, each drawn under a transform
    /// of the linear part of `transform`. `None` when it draws nothing.
    fn push_shape(
        &mut self,
        drawable: &Use,
        geometry: &Geometry,
        transform: &GpuDrawableTransform,
        (kind, size, border_radius): (ShapeGeometryKind, [f32; 2], f32),
    ) -> Option<Shared> {
        if size.contains(&0.0) {
            return None;
        }
        let smallest = scales(transform)[0].max(1e-12);
        let offset = geometry.offset;
        let (flags, stroke_width, grow, feather) = match drawable.style {
            Style::Fill(fill) => (
                style_flags(true, false, fill.edge_antialias),
                0.0,
                offset.max(0.0) + fill.feather * 0.5,
                fill.feather,
            ),
            Style::Stroke(stroke) => {
                if stroke.width <= 0.0 {
                    return None;
                }
                let half = stroke.width * 0.5;
                if offset + half <= 0.0 && -offset - half >= size[0].min(size[1]) * 0.5 {
                    return None;
                }
                (
                    style_flags(false, true, true),
                    stroke.width,
                    (offset + half).max(0.0),
                    0.0,
                )
            }
        };
        let (mut shadow_softness, mut shadow_spread, mut shadow_flag, mut reach) =
            (0.0, 0.0, 0, 0.0);
        if let Some(cast) = drawable.shadow {
            // A feathered fill's shadow fades over the feather too.
            shadow_softness = cast.softness() + f64::from(feather) * smallest;
            shadow_spread = cast.spread;
            shadow_flag = STYLE_SHADOW;
            reach = ((cast.spread.max(0.0) + shadow_softness * 0.5) / smallest) as f32;
        }
        let sheared = if measured(*transform).turn.is_some() {
            DRAWABLE_SHEARED
        } else {
            0
        };
        let position = geometry.position;
        // The box the shader draws in, at least a pixel across, and what is drawn beyond it.
        let drawn = size.map(|side| side.max(1.0));
        let grow = grow + reach;
        let shadow_paint = self.shadow_paint(drawable);
        self.drawables.push(GpuVectorDrawable {
            paint_frame: [[1.0, 0.0], [0.0, 1.0]],
            paint_min: position,
            paint_size: size,
            position,
            size,
            kind: DRAWABLE_SHAPE,
            flags: flags | shadow_flag | sheared,
            stroke_width,
            fill_offset: offset,
            geometry_kind: kind as u32,
            border_radius,
            paint: drawable.paint,
            shadow_softness: shadow_softness as f32,
            shadow_spread: shadow_spread as f32,
            fill_feather: feather,
            shadow_paint,
            ..Zeroable::zeroed()
        });
        Some(Shared {
            drawable: self.drawables.len() as u32 - 1,
            low: [position[0] - grow, position[1] - grow],
            high: [position[0] + drawn[0] + grow, position[1] + drawn[1] + grow],
            // Antialiasing reaches a canvas pixel beyond the box.
            margin: 1.0,
        })
    }

    /// Index in the paints buffer of the colour of `drawable`'s shadow, or 0 when it is none.
    fn shadow_paint(&mut self, drawable: &Use) -> u32 {
        drawable
            .shadow
            .map_or(0, |cast| self.paint(&GpuPaint::solid(cast.color)))
    }
}

/// A drawable that copies share, and the untransformed box each copy is drawn within, past
/// which a copy draws `margin` canvas pixels.
#[derive(Clone, Copy)]
struct Shared {
    drawable: u32,
    low: [f32; 2],
    high: [f32; 2],
    margin: f32,
}

impl Shared {
    /// The copy of this drawable placed by `transform` and tinted by `tint`.
    fn copy(&self, transform: GpuDrawableTransform, tint: [f32; 3]) -> GpuVectorCopy {
        let [min_x, min_y, max_x, max_y] = transform.canvas_bounds(self.low, self.high);
        let margin = self.margin;
        GpuVectorCopy {
            transform,
            screen_bounds: [
                min_x - margin,
                min_y - margin,
                max_x + margin,
                max_y + margin,
            ],
            tint,
            drawable: self.drawable,
        }
    }
}

/// `transform`, where `drawable`'s outline is placed, moved by its shadow's offset and at its
/// opacity.
fn copy_transform(drawable: &Use, mut transform: GpuDrawableTransform) -> GpuDrawableTransform {
    transform.opacity *= drawable.opacity;
    if let Some(cast) = drawable.shadow {
        transform = transform.translated(cast.offset);
        transform.opacity *= cast.opacity;
    }
    transform
}

/// What decides the drawable a copy draws: the drawable's origin, the linear part of the copy's
/// placement and the lengths of its shadow.
type SharedKey = (u32, Option<usize>, [u32; 8], [u64; 2]);

/// Builds the buffers for `record`, drawn on a canvas of `canvas` pixels, with the glyphs each
/// text node in it was shaped as in `texts`. Paths from nodes `keeps` says don't read the frame
/// are kept for later frames. `timed` starts timing a path or text node, which its paths are
/// built under.
pub(super) fn build<'t>(
    record: &Record,
    texts: &HashMap<NodeId, Arc<TextGlyphs>>,
    keeps: &dyn Fn(NodeId) -> bool,
    timed: &dyn Fn(NodeId) -> Timed<'t>,
    canvas: [f32; 2],
    keeping: &mut Keeping<'_>,
) -> crate::Result<Drawing> {
    let mut drawing = Drawing {
        copies: Vec::with_capacity(record.uses.len()),
        paints: record.paints.clone(),
        ..Drawing::default()
    };
    // Each drawable's placement and opacity, or `None` when it draws nothing.
    let placed: Vec<Option<GpuDrawableTransform>> = record
        .uses
        .iter()
        .map(|drawable| {
            let geometry = &record.geometries[drawable.geometry];
            let transform = then(geometry.transform, drawable.parent);
            let visible =
                transform.opacity * drawable.opacity > 0.0 && scales(&transform)[0] >= MIN_SCALE;
            visible.then_some(transform)
        })
        .collect();
    let grouped = instances(record, &placed);
    let instances = grouped.instances;

    let mut parsed: HashMap<(NodeId, u32), (Vec<PathPoint>, usize)> = HashMap::default();
    let tolerance =
        |instance: &Instance| flattening_tolerance((instance.scales[1], instance.scales[1]));
    for instance in &instances {
        if let Source::Path { node, .. } = record.geometries[instance.geometry].source {
            parsed
                .entry((node, tolerance(instance).to_bits()))
                .or_default()
                .1 += 1;
        }
    }
    // The paths each instance is drawn from, placed in the drawing's buffers.
    let mut placed_paths: Vec<Vec<PlacedPath>> = Vec::with_capacity(instances.len());
    let mut ordinals: HashMap<NodeId, usize> = HashMap::default();
    for instance in &instances {
        let geometry = &record.geometries[instance.geometry];
        let node = geometry
            .source
            .path_node()
            .expect("only paths and texts are instanced");
        let _timed = timed(node);
        let ordinal = ordinals.entry(node).or_default();
        let slot = (keeping.scene, keeping.render, node, *ordinal);
        *ordinal += 1;
        let text = matches!(geometry.source, Source::Text { .. }).then(|| &texts[&node]);
        let key = path_key(
            record,
            instance,
            &grouped.of,
            canvas,
            text.map_or(0, |text| text.id),
        );
        let keep = keeps(node);
        let turn = instance.measured.turn;
        if keep
            && let Some(kept) = keeping.cache.paths.get_mut(&slot)
            && kept.key == key
        {
            kept.last_used = keeping.tick;
            placed_paths.push(
                kept.built
                    .iter()
                    .map(|path| drawing.place(path, turn))
                    .collect(),
            );
            continue;
        }
        let built: Vec<BuiltPath> = match (&geometry.source, text) {
            (Source::Path { data, .. }, _) => {
                let points = take_points(&mut parsed, node, tolerance(instance), data)?;
                let bounds = point_bounds(&points);
                vec![build_path(
                    record,
                    instance,
                    &grouped.of,
                    geometry,
                    points,
                    bounds,
                    canvas,
                )]
            }
            (Source::Text { .. }, Some(text)) => {
                let paths = keeping
                    .cache
                    .texts
                    .paths(text, tolerance(instance), keeping.tick)
                    .map_err(|details| RenderError::evaluating::<Text2d>(node, details))?;
                paths
                    .chunks
                    .iter()
                    .map(|points| {
                        build_path(
                            record,
                            instance,
                            &grouped.of,
                            geometry,
                            points.clone(),
                            paths.bounds,
                            canvas,
                        )
                    })
                    .collect()
            }
            _ => unreachable!("only paths and texts are instanced"),
        };
        placed_paths.push(built.iter().map(|path| drawing.place(path, turn)).collect());
        if keep {
            keeping.cache.paths.insert(
                slot,
                KeptPath {
                    key,
                    built,
                    last_used: keeping.tick,
                },
            );
        }
    }

    // The drawables made so far, and those each key draws.
    let mut shared: Vec<Shared> = Vec::new();
    let mut sharing: HashMap<SharedKey, Range<usize>> = HashMap::default();
    for (index, drawable) in record.uses.iter().enumerate() {
        let Some(transform) = placed[index] else {
            continue;
        };
        let geometry = &record.geometries[drawable.geometry];
        let (instance, measured) = grouped.of[index];
        // A path or text drawable shares its group's built outline, which is turned for that
        // group's placement, so copies share a drawable only within one group.
        let (outline, transform) = match geometry.source {
            Source::Path { .. } | Source::Text { .. } => {
                (Some(instance), copy_transform(drawable, measured))
            }
            Source::Shape { .. } => (None, copy_transform(drawable, transform)),
        };
        let cast = drawable
            .shadow
            .map_or([0; 2], |cast| [cast.sigma.to_bits(), cast.spread.to_bits()]);
        let key = (drawable.origin, outline, linear_key(&transform), cast);
        let range = match sharing.get(&key) {
            Some(range) => range.clone(),
            None => {
                let start = shared.len();
                match geometry.source {
                    Source::Path { .. } | Source::Text { .. } => {
                        shared.extend(placed_paths[instance].iter().filter_map(|path| {
                            drawing.push_path(drawable, geometry.offset, &instances[instance], path)
                        }));
                    }
                    Source::Shape {
                        kind,
                        size,
                        border_radius,
                    } => shared.extend(drawing.push_shape(
                        drawable,
                        geometry,
                        &transform,
                        (kind, size, border_radius),
                    )),
                }
                sharing.insert(key, start..shared.len());
                start..shared.len()
            }
        };
        drawing.copies.extend(
            shared[range]
                .iter()
                .map(|shared| shared.copy(transform, drawable.tint)),
        );
        if drawing.copies.len() > MAX_DRAWABLES {
            return Err(RenderError::evaluating::<Render2d>(
                keeping.render,
                format!("the drawing holds more than {MAX_DRAWABLES} drawables"),
            )
            .into());
        }
    }
    Ok(drawing)
}

type InstanceKey = (usize, [u32; 8], Option<(u64, u64)>);

/// The drawn path drawables grouped by outline and linear part of their placements.
struct Grouped {
    instances: Vec<Instance>,
    /// Each path drawable's group and the transform the shaders measure it with.
    of: Vec<(usize, GpuDrawableTransform)>,
}

/// Groups the drawn path drawables by outline, linear part of their placement and, for dashed
/// strokes, the dash their points are measured for.
fn instances(record: &Record, placed: &[Option<GpuDrawableTransform>]) -> Grouped {
    let mut instances: Vec<Instance> = Vec::new();
    let mut of = vec![(usize::MAX, identity()); record.uses.len()];
    let mut found: HashMap<InstanceKey, usize> = HashMap::default();
    let mut placements: HashSet<(usize, [u32; 4])> =
        HashSet::with_capacity_and_hasher(record.uses.len(), Default::default());
    for (index, drawable) in record.uses.iter().enumerate() {
        let Some(transform) = placed[index] else {
            continue;
        };
        if record.geometries[drawable.geometry]
            .source
            .path_node()
            .is_none()
        {
            continue;
        }
        let linear = linear_key(&transform);
        let dash = match drawable.style {
            Style::Stroke(stroke) if stroke.dash.count > 0 => Some(stroke.dash),
            _ => None,
        };
        // Fills and solid strokes share the first group of their linear part; a dashed stroke
        // needs the points measured for its own dash.
        let first = found.get(&(drawable.geometry, linear, None)).copied();
        let slot = match (dash, first) {
            (None, first) => first,
            (Some(dash), Some(first)) if instances[first].dash.is_none() => {
                instances[first].dash = Some(dash);
                found.insert((drawable.geometry, linear, Some(dash.phase_key())), first);
                Some(first)
            }
            (Some(dash), _) => found
                .get(&(drawable.geometry, linear, Some(dash.phase_key())))
                .copied(),
        };
        // Every group of one linear part measures it the same way.
        let measured = first.map_or_else(
            || measured(transform),
            |first| instances[first].measured.moved(transform),
        );
        let slot = slot.unwrap_or_else(|| {
            let slot = instances.len();
            instances.push(Instance {
                geometry: drawable.geometry,
                measured,
                transforms: Vec::new(),
                scales: scales(&measured.transform),
                dash,
                uses: Vec::new(),
            });
            found
                .entry((drawable.geometry, linear, None))
                .or_insert(slot);
            if let Some(dash) = dash {
                found.insert((drawable.geometry, linear, Some(dash.phase_key())), slot);
            }
            slot
        });
        let ([x, y], [inverse_x, inverse_y]) = (transform.offset, transform.inverse_offset);
        let offsets = [x, y, inverse_x, inverse_y].map(f32::to_bits);
        if placements.insert((slot, offsets)) {
            instances[slot].transforms.push(opaque(measured.transform));
        }
        instances[slot].uses.push(index);
        of[index] = (slot, measured.transform);
    }
    Grouped { instances, of }
}

fn path_key(
    record: &Record,
    instance: &Instance,
    of: &[(usize, GpuDrawableTransform)],
    canvas: [f32; 2],
    text: u64,
) -> PathKey {
    let geometry = &record.geometries[instance.geometry];
    let uses = instance.uses.iter().map(|&index| &record.uses[index]);
    PathKey {
        text,
        transforms: instance.transforms.clone(),
        casts: shadow_transforms(record, instance, of),
        needs: uses
            .filter_map(|drawable| path_need(drawable, geometry.offset, instance.scales))
            .collect(),
        dash: instance.dash.as_ref().map(GpuDash::phase_key),
        canvas,
    }
}

/// The points of path node `node` flattened at `tolerance`, parsing them on the first call.
fn take_points(
    parsed: &mut HashMap<(NodeId, u32), (Vec<PathPoint>, usize)>,
    node: NodeId,
    tolerance: f32,
    data: &str,
) -> crate::Result<Vec<PathPoint>> {
    let key = (node, tolerance.to_bits());
    let entry = parsed.get_mut(&key).expect("every instance is counted");
    if entry.0.is_empty() {
        entry.0 = parse_path_points(data, tolerance, MAX_PATH_POINTS).map_err(|details| {
            RenderError::evaluating::<Path2d>(node, format!("`data` {details}"))
        })?;
    }
    entry.1 -= 1;
    Ok(if entry.1 == 0 {
        parsed
            .remove(&key)
            .map(|(points, _)| points)
            .unwrap_or_default()
    } else {
        entry.0.clone()
    })
}

/// The transforms the shaders draw the shadows among `instance`'s drawables with, at full
/// opacity.
fn shadow_transforms(
    record: &Record,
    instance: &Instance,
    of: &[(usize, GpuDrawableTransform)],
) -> Vec<GpuDrawableTransform> {
    instance
        .uses
        .iter()
        .filter_map(|&index| {
            let cast = record.uses[index].shadow?;
            Some(opaque(of[index].1.translated(cast.offset)))
        })
        .collect()
}

/// Measures and bins the outline of `instance` for every drawable it draws, from its flattened
/// `points`, with a paint in bounding-box units spanning `paint_bounds`, a box of untransformed
/// points.
#[allow(clippy::too_many_arguments)]
fn build_path(
    record: &Record,
    instance: &Instance,
    of: &[(usize, GpuDrawableTransform)],
    geometry: &Geometry,
    mut points: Vec<PathPoint>,
    paint_bounds: ([f32; 2], [f32; 2]),
    canvas: [f32; 2],
) -> BuiltPath {
    let needs: Vec<(bool, PathNeed)> = instance
        .uses
        .iter()
        .filter_map(|&index| {
            let drawable = &record.uses[index];
            let need = path_need(drawable, geometry.offset, instance.scales)?;
            Some((drawable.shadow.is_some(), need))
        })
        .collect();
    if needs.iter().any(|(_, need)| need.fill_span.is_some()) {
        hide_cancelled_edges(&mut points, MAX_PATH_POINTS);
    }
    if let Some(dash) = &instance.dash {
        set_dash_phases(&mut points, dash);
    }
    let (low, high) = paint_bounds;
    let paint_box = (
        [geometry.position[0] + low[0], geometry.position[1] + low[1]],
        [(high[0] - low[0]).max(1.0), (high[1] - low[1]).max(1.0)],
    );
    let mut position = geometry.position;
    if let Some(turn) = instance.measured.turn {
        let turned = |point: [f32; 2]| apply64(turn, point.map(f64::from)).map(|v| v as f32);
        for point in &mut points {
            point.position = turned(point.position);
        }
        position = turned(position);
    }
    let bounds = point_bounds(&points);

    let shadows = shadow_transforms(record, instance, of);
    let drawn = [
        Drawn {
            transforms: &instance.transforms,
            scales: instance.scales,
        },
        Drawn {
            transforms: &shadows,
            scales: instance.scales,
        },
    ];
    let mut grids = [false, true].map(|shadow| {
        let mut needs = needs
            .iter()
            .filter(|(cast, _)| *cast == shadow)
            .map(|(_, need)| need)
            .peekable();
        needs.peek()?;
        Some(path_grid(needs, position))
    });
    let reaches: Vec<Option<GridReach>> = grids
        .iter_mut()
        .zip(&drawn)
        .map(|(uniforms, drawn)| GridReach::new(uniforms.as_mut()?, bounds, drawn))
        .collect();
    let draws = reaches
        .iter()
        .flatten()
        .fold([false; 2], |[fill, stroke], reach| {
            let [f, s] = reach.draws();
            [fill || f, stroke || s]
        });
    let reach = reaches
        .iter()
        .flatten()
        .map(GridReach::reach)
        .fold(0.0, f64::max);
    let capacity = band_capacity(MAX_PATH_POINTS);
    let pixel = 1.0 / instance.scales[0].max(MIN_SCALE);
    let bins = PathBins::new(&points, bounds, draws, pixel, reach, capacity);
    let mut built_grids = [None, None];
    for (slot, built) in built_grids.iter_mut().enumerate() {
        let (Some(uniforms), Some(reach)) = (grids[slot].as_mut(), reaches[slot].as_ref()) else {
            continue;
        };
        let Some((size, data)) =
            bins.grid(uniforms, reach, &points, &drawn[slot], canvas, 0, capacity)
        else {
            continue;
        };
        *built = Some(BuiltGrid {
            fields: GridFields {
                band_origin: uniforms.band_origin,
                band_scale: uniforms.band_scale,
                band_count: uniforms.band_count,
                grid_origin: uniforms.grid_origin,
                grid_scale: uniforms.grid_scale,
                grid_size: uniforms.grid_size,
                reach: uniforms.reach,
            },
            pairs: 2 * (size[0] * size[1]) as usize,
            data,
        });
    }
    BuiltPath {
        band_pairs: bins.band_pairs(),
        bands: bins.data,
        points,
        grids: built_grids,
        position,
        bounds,
        paint_box,
    }
}

/// Whether `copy` hides what is below it wherever it fully covers a pixel.
pub(super) fn is_opaque(drawing: &Drawing, copy: &GpuVectorCopy) -> bool {
    let drawable = &drawing.drawables[copy.drawable as usize];
    let paint = &drawing.paints[drawable.paint as usize];
    copy.transform.opacity >= 1.0
        && drawable.flags & STYLE_FILL_ENABLED != 0
        && paint.colors[..paint.stop_count as usize]
            .iter()
            .all(|color| color[3] >= 1.0)
}

/// Whether `drawable` is a shape, an ellipse when the second is true.
pub(super) fn shape_kind(drawable: &GpuVectorDrawable) -> Option<bool> {
    (drawable.kind == DRAWABLE_SHAPE)
        .then_some(drawable.geometry_kind == ShapeGeometryKind::Ellipse as u32)
}
