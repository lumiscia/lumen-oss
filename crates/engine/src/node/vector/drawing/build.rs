//! Turns a drawing's record into the buffers `render_2d` draws from. Each path outline is
//! flattened and banded once for all of its drawables, which read one grid for the fills and
//! strokes and another for the shadows.

use std::hash::BuildHasher;

use bytemuck::{Pod, Zeroable};
use foldhash::HashMap;

use super::super::dash::GpuDash;
use super::super::drawable::{DrawableTransform, GpuDrawableTransform, MIN_SCALE, union_bounds};
use super::super::paint::GpuPaint;
use super::super::path::{
    flattening_tolerance, hide_cancelled_edges, parse_path_points, point_bounds, set_dash_phases,
};
use super::super::path_bands::{GridReach, PathBins, band_capacity};
use super::super::renderer::{
    MAX_PATH_POINTS, PathPoint, PathUniforms, STYLE_FILL_ENABLED, STYLE_STROKE_ENABLED, style_flags,
};
use super::super::repeat::Drawn;
use super::super::shape::ShapeGeometryKind;
use super::Path2d;
use super::collect::{Geometry, Record, Source, Style, Use};
use crate::composition::SceneId;
use crate::error::RenderError;
use crate::gpu::frame_graph::Timed;
use crate::node::NodeId;

pub(super) const DRAWING_SHADER: &str = concat!(
    include_str!("../../../gpu/shaders/alpha.wgsl"),
    include_str!("../../../gpu/shaders/color.wgsl"),
    include_str!("../paint/paint.wgsl"),
    include_str!("../drawable.wgsl"),
    include_str!("../vector_common.wgsl"),
    include_str!("../path_geometry.wgsl"),
    include_str!("../shape_geometry.wgsl"),
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
    pub(super) transform: GpuDrawableTransform,
    /// Canvas-space `[min_x, min_y, max_x, max_y]` outside which nothing is drawn.
    pub(super) screen_bounds: [f32; 4],
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

/// The drawables of a drawing, lowest first, and the buffers they index.
#[derive(Default)]
pub(super) struct Drawing {
    pub(super) drawables: Vec<GpuVectorDrawable>,
    /// Every path's points, each contour's start index counted from the first path's.
    pub(super) points: Vec<PathPoint>,
    /// Every path's bands and grids, each index into them or into `points` counted from the
    /// start of the buffer.
    pub(super) bands: Vec<u32>,
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
    let inverse_offset = apply64(turn, transform.inverse_offset.map(f64::from));
    let to_f32 = |m: [[f64; 2]; 2]| m.map(|column| column.map(|value| value as f32));
    Measured {
        transform: GpuDrawableTransform {
            linear: to_f32(linear),
            inverse_linear: to_f32(inverse_linear),
            inverse_offset: inverse_offset.map(|value| value as f32),
            ..transform
        },
        turn: Some(turn),
    }
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

/// The uniforms a grid shared by `needs`, laid out from `position`, is binned from: a fill
/// whose coverage varies across every need's span, and a stroke as wide as the widest.
fn grid_uniforms<'a>(
    needs: impl Iterator<Item = &'a PathNeed>,
    position: [f32; 2],
) -> PathUniforms {
    let mut uniforms = PathUniforms::zeroed();
    uniforms.position = position;
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
    transform: GpuDrawableTransform,
    casts: Vec<[f32; 2]>,
    needs: Vec<PathNeed>,
    dash: Option<(u64, u64)>,
    canvas: [f32; 2],
}

struct KeptPath {
    key: PathKey,
    built: BuiltPath,
    last_used: u64,
}

/// Paths `render_2d` built on earlier frames from path nodes whose params don't read the frame,
/// kept while later frames draw them the same way.
#[derive(Default)]
pub(crate) struct DrawingCache {
    paths: HashMap<(SceneId, NodeId, NodeId, usize), KeptPath>,
}

impl DrawingCache {
    /// Drops the paths that neither this frame nor the last one drew.
    pub(crate) fn begin_frame(&mut self, tick: u64) {
        self.paths
            .retain(|_, kept| kept.last_used.saturating_add(1) >= tick);
    }

    pub(crate) fn clear(&mut self) {
        self.paths.clear();
    }
}

/// Where in the cache a `render_2d` keeps its paths, and the frame it records.
pub(crate) struct Keeping<'a> {
    pub(crate) cache: &'a mut DrawingCache,
    pub(crate) scene: SceneId,
    pub(crate) render: NodeId,
    pub(crate) tick: u64,
}

/// The drawables of one outline under one placement, and the dash its points are measured for.
struct Instance {
    geometry: usize,
    /// The placement, at full opacity.
    measured: Measured,
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

/// The bits of a transform's linear part and offset, which drawables placed alike share.
fn placement_key(transform: &GpuDrawableTransform) -> [u32; 12] {
    let mut key = [0; 12];
    let values = transform
        .linear
        .iter()
        .chain(&transform.inverse_linear)
        .flatten()
        .chain(&transform.offset)
        .chain(&transform.inverse_offset);
    for (slot, value) in key.iter_mut().zip(values) {
        *slot = value.to_bits();
    }
    key
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
        self.drawables
            .iter()
            .map(|drawable| drawable.screen_bounds)
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

    /// Adds `drawable`, drawn under `transform` with the outline `path` placed.
    fn push_path(&mut self, drawable: &Use, offset: f32, instance: &Instance, path: &PlacedPath) {
        let Some(need) = path_need(drawable, offset, instance.scales) else {
            return;
        };
        let shadow = need.flags & STYLE_SHADOW != 0;
        let Some((grid, grid_start)) = path.grids[usize::from(shadow)] else {
            return;
        };
        let mut transform = instance.measured.transform;
        transform.opacity = drawable.opacity * instance.measured.transform.opacity;
        let mut shadow_paint = None;
        if let Some(cast) = drawable.shadow {
            transform = transform.translated(cast.offset);
            transform.opacity *= cast.opacity;
            shadow_paint = Some(GpuPaint::solid(cast.color));
        }
        let extent = need.extent + (1.0 / instance.scales[0].max(MIN_SCALE)) as f32;
        let [x, y] = path.position;
        let (min, max) = path.bounds;
        let low = [min[0] - extent + x, min[1] - extent + y];
        let high = [max[0] + extent + x, max[1] + extent + y];
        if low[0] > high[0] || low[1] > high[1] {
            return;
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
        let paint = self.paint(&drawable.paint);
        let shadow_paint = shadow_paint.map_or(0, |shadow| self.paint(&shadow));
        self.drawables.push(GpuVectorDrawable {
            transform,
            screen_bounds: transform.canvas_bounds(low, high),
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
            paint,
            shadow_softness: need.shadow_softness,
            dash_count: dash.count,
            dash_period: dash.period(),
            dash_start_phase: dash.phase(0.0),
            dash: dash.lengths,
            shadow_paint,
            ..Zeroable::zeroed()
        });
    }

    /// Adds `drawable` of shape `geometry`, drawn under `transform`.
    fn push_shape(
        &mut self,
        drawable: &Use,
        geometry: &Geometry,
        mut transform: GpuDrawableTransform,
        (kind, size, border_radius): (ShapeGeometryKind, [f32; 2], f32),
    ) {
        if size.contains(&0.0) {
            return;
        }
        let smallest = scales(&transform)[0].max(1e-12);
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
                    return;
                }
                let half = stroke.width * 0.5;
                if offset + half <= 0.0 && -offset - half >= size[0].min(size[1]) * 0.5 {
                    return;
                }
                (
                    style_flags(false, true, true),
                    stroke.width,
                    (offset + half).max(0.0),
                    0.0,
                )
            }
        };
        let mut shadow_paint = None;
        let (mut shadow_softness, mut shadow_spread, mut shadow_flag, mut reach) =
            (0.0, 0.0, 0, 0.0);
        if let Some(cast) = drawable.shadow {
            transform = transform.translated(cast.offset);
            transform.opacity *= cast.opacity;
            shadow_paint = Some(GpuPaint::solid(cast.color));
            // A feathered fill's shadow fades over the feather too.
            shadow_softness = cast.softness() + f64::from(feather) * smallest;
            shadow_spread = cast.spread;
            shadow_flag = STYLE_SHADOW;
            reach = ((cast.spread.max(0.0) + shadow_softness * 0.5) / smallest) as f32;
        }
        let sheared = if measured(transform).turn.is_some() {
            DRAWABLE_SHEARED
        } else {
            0
        };
        let position = geometry.position;
        // The box the shader draws in, at least a pixel across, and what is drawn beyond it.
        let drawn = size.map(|side| side.max(1.0));
        let grow = grow + reach;
        let [min_x, min_y, max_x, max_y] = transform.canvas_bounds(
            [position[0] - grow, position[1] - grow],
            [position[0] + drawn[0] + grow, position[1] + drawn[1] + grow],
        );
        let paint = self.paint(&drawable.paint);
        let shadow_paint = shadow_paint.map_or(0, |shadow| self.paint(&shadow));
        self.drawables.push(GpuVectorDrawable {
            transform,
            // Antialiasing reaches a canvas pixel beyond the box.
            screen_bounds: [min_x - 1.0, min_y - 1.0, max_x + 1.0, max_y + 1.0],
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
            paint,
            shadow_softness: shadow_softness as f32,
            shadow_spread: shadow_spread as f32,
            fill_feather: feather,
            shadow_paint,
            ..Zeroable::zeroed()
        });
    }
}

/// Builds the buffers for `record`, drawn on a canvas of `canvas` pixels. Paths from nodes
/// `keeps` says don't read the frame are kept for later frames. `timed` starts timing a path
/// node, which its paths are built under.
pub(super) fn build<'t>(
    record: &Record,
    keeps: &dyn Fn(NodeId) -> bool,
    timed: &dyn Fn(NodeId) -> Timed<'t>,
    canvas: [f32; 2],
    keeping: &mut Keeping<'_>,
) -> crate::Result<Drawing> {
    let mut drawing = Drawing::default();
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
    let (instances, instance_of) = instances(record, &placed);

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
    let mut placed_paths = Vec::with_capacity(instances.len());
    let mut ordinals: HashMap<NodeId, usize> = HashMap::default();
    for instance in &instances {
        let geometry = &record.geometries[instance.geometry];
        let Source::Path { node, data } = &geometry.source else {
            unreachable!("only paths are instanced")
        };
        let _timed = timed(*node);
        let ordinal = ordinals.entry(*node).or_default();
        let slot = (keeping.scene, keeping.render, *node, *ordinal);
        *ordinal += 1;
        let key = path_key(record, instance, canvas);
        let keep = keeps(*node);
        if keep
            && let Some(kept) = keeping.cache.paths.get_mut(&slot)
            && kept.key == key
        {
            kept.last_used = keeping.tick;
            placed_paths.push(drawing.place(&kept.built, instance.measured.turn));
            continue;
        }
        let points = take_points(&mut parsed, *node, tolerance(instance), data)?;
        let path = build_path(record, instance, geometry, points, canvas);
        placed_paths.push(drawing.place(&path, instance.measured.turn));
        if keep {
            keeping.cache.paths.insert(
                slot,
                KeptPath {
                    key,
                    built: path,
                    last_used: keeping.tick,
                },
            );
        }
    }

    for (index, drawable) in record.uses.iter().enumerate() {
        let Some(transform) = placed[index] else {
            continue;
        };
        let geometry = &record.geometries[drawable.geometry];
        match geometry.source {
            Source::Path { .. } => {
                let instance = instance_of[index];
                drawing.push_path(
                    drawable,
                    geometry.offset,
                    &instances[instance],
                    &placed_paths[instance],
                );
            }
            Source::Shape {
                kind,
                size,
                border_radius,
            } => {
                let transform = GpuDrawableTransform {
                    opacity: transform.opacity * drawable.opacity,
                    ..transform
                };
                drawing.push_shape(drawable, geometry, transform, (kind, size, border_radius));
            }
        }
    }
    Ok(drawing)
}

type InstanceKey = (usize, [u32; 12], Option<(u64, u64)>);

/// Groups the drawn path drawables by outline and placement, and gives each drawable's group.
fn instances(
    record: &Record,
    placed: &[Option<GpuDrawableTransform>],
) -> (Vec<Instance>, Vec<usize>) {
    let mut instances: Vec<Instance> = Vec::new();
    let mut instance_of = vec![usize::MAX; record.uses.len()];
    // By outline, placement and, for dashed strokes, the dash their points are measured for.
    let mut found: HashMap<InstanceKey, usize> = HashMap::default();
    for (index, drawable) in record.uses.iter().enumerate() {
        let Some(transform) = placed[index] else {
            continue;
        };
        if !matches!(
            record.geometries[drawable.geometry].source,
            Source::Path { .. }
        ) {
            continue;
        }
        let placement = placement_key(&transform);
        let dash = match drawable.style {
            Style::Stroke(stroke) if stroke.dash.count > 0 => Some(stroke.dash),
            _ => None,
        };
        // Fills and solid strokes share the placement's first group; a dashed stroke needs the
        // points measured for its own dash.
        let first = found.get(&(drawable.geometry, placement, None)).copied();
        let slot = match (dash, first) {
            (None, first) => first,
            (Some(dash), Some(first)) if instances[first].dash.is_none() => {
                instances[first].dash = Some(dash);
                found.insert(
                    (drawable.geometry, placement, Some(dash.phase_key())),
                    first,
                );
                Some(first)
            }
            (Some(dash), _) => found
                .get(&(drawable.geometry, placement, Some(dash.phase_key())))
                .copied(),
        };
        let slot = slot.unwrap_or_else(|| {
            let slot = instances.len();
            let measured = measured(transform);
            instances.push(Instance {
                geometry: drawable.geometry,
                measured,
                scales: scales(&measured.transform),
                dash,
                uses: Vec::new(),
            });
            found
                .entry((drawable.geometry, placement, None))
                .or_insert(slot);
            if let Some(dash) = dash {
                found.insert((drawable.geometry, placement, Some(dash.phase_key())), slot);
            }
            slot
        });
        instances[slot].uses.push(index);
        instance_of[index] = slot;
    }
    (instances, instance_of)
}

fn path_key(record: &Record, instance: &Instance, canvas: [f32; 2]) -> PathKey {
    let geometry = &record.geometries[instance.geometry];
    let uses = instance.uses.iter().map(|&index| &record.uses[index]);
    PathKey {
        transform: instance.measured.transform,
        casts: uses
            .clone()
            .filter_map(|drawable| drawable.shadow.map(|cast| cast.offset))
            .collect(),
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

/// Measures and bins the outline of `instance` for every drawable it draws, from its flattened
/// `points`.
fn build_path(
    record: &Record,
    instance: &Instance,
    geometry: &Geometry,
    mut points: Vec<PathPoint>,
    canvas: [f32; 2],
) -> BuiltPath {
    let needs: Vec<(Option<[f32; 2]>, PathNeed)> = instance
        .uses
        .iter()
        .filter_map(|&index| {
            let drawable = &record.uses[index];
            let need = path_need(drawable, geometry.offset, instance.scales)?;
            Some((drawable.shadow.map(|cast| cast.offset), need))
        })
        .collect();
    if needs.iter().any(|(_, need)| need.fill_span.is_some()) {
        hide_cancelled_edges(&mut points, MAX_PATH_POINTS);
    }
    if let Some(dash) = &instance.dash {
        set_dash_phases(&mut points, dash);
    }
    let original = point_bounds(&points);
    let paint_box = (
        [
            geometry.position[0] + original.0[0],
            geometry.position[1] + original.0[1],
        ],
        [
            (original.1[0] - original.0[0]).max(1.0),
            (original.1[1] - original.0[1]).max(1.0),
        ],
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

    let transform = instance.measured.transform;
    let body = [transform];
    let shadows: Vec<GpuDrawableTransform> = needs
        .iter()
        .filter_map(|(cast, _)| cast.map(|offset| transform.translated(offset)))
        .collect();
    let drawn = [
        Drawn::Affine(&body, instance.scales),
        Drawn::Affine(&shadows, instance.scales),
    ];
    let mut grids = [false, true].map(|shadow| {
        let mut needs = needs
            .iter()
            .filter(|(cast, _)| cast.is_some() == shadow)
            .map(|(_, need)| need)
            .peekable();
        needs.peek()?;
        Some(grid_uniforms(needs, position))
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

/// Whether drawable `index` hides what is below it wherever it fully covers a pixel.
pub(super) fn is_opaque(drawing: &Drawing, index: usize) -> bool {
    let drawable = &drawing.drawables[index];
    let paint = &drawing.paints[drawable.paint as usize];
    drawable.transform.opacity >= 1.0
        && drawable.flags & STYLE_FILL_ENABLED != 0
        && paint.colors[..paint.stop_count as usize]
            .iter()
            .all(|color| color[3] >= 1.0)
}

/// Whether drawable `index` is a shape, an ellipse when the second is true.
pub(super) fn shape_kind(drawable: &GpuVectorDrawable) -> Option<bool> {
    (drawable.kind == DRAWABLE_SHAPE)
        .then_some(drawable.geometry_kind == ShapeGeometryKind::Ellipse as u32)
}
