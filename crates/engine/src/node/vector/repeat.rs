use bytemuck::{Pod, Zeroable};
use serde_json::Value;

use super::drawable::{DrawableMotion, DrawableTransform, GpuDrawableTransform, union_bounds};
use crate::error::LumenError;
use crate::expr::{ExpressionContext, ExpressionValue};
use crate::node::motion::{self, Shutter};
use crate::node::{
    Deferred, DelegateEvalContext, DelegateEvaluable, Delegated, NodeId, ParamValue, truncated_int,
};

/// Most copies a repeat may draw.
pub const MAX_REPEAT_COUNT: u32 = 1024;

/// Side in texels of the smallest tiles that a repeat's copies are binned into.
const MIN_TILE_SIZE: u32 = 16;
/// Most tiles a repeat's texture is split into.
const MAX_TILES: u64 = 4096;
/// Most copy indices that a repeat's tiles list in all. One tile listing every copy fits.
const MAX_TILE_ENTRIES: u64 = 1 << 16;
const _: () = assert!(MAX_TILE_ENTRIES >= MAX_REPEAT_COUNT as u64);

/// How many copies a repeat draws, from 0 to [`MAX_REPEAT_COUNT`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, lumen_macros::StructuralHash)]
pub struct RepeatCount(pub u32);

impl RepeatCount {
    fn new(count: i64) -> Option<Self> {
        u32::try_from(count)
            .ok()
            .filter(|count| *count <= MAX_REPEAT_COUNT)
            .map(Self)
    }
}

impl ParamValue for RepeatCount {
    const ACCEPTS_EXPRESSIONS: bool = true;

    fn from_json_value(value: &Value, path: &str) -> Result<Self, String> {
        let count = i64::from_json_value(value, path)?;
        Self::new(count).ok_or_else(|| {
            format!("`{path}` must be an integer from 0 to {MAX_REPEAT_COUNT}, got {count}")
        })
    }

    fn to_json_value(&self) -> Value {
        Value::from(self.0)
    }

    fn from_expression(value: ExpressionValue) -> crate::Result<Self> {
        let count = truncated_int(value)?;
        Self::new(count).ok_or_else(|| {
            LumenError::evaluate(format!(
                "{count} copies is out of range; a repeat draws 0 to {MAX_REPEAT_COUNT}"
            ))
        })
    }

    fn to_expression(&self) -> Option<ExpressionValue> {
        Some(ExpressionValue::Number(f64::from(self.0)))
    }
}

impl Delegated for RepeatCount {
    type Delegate = Deferred<RepeatCount>;

    fn into_delegate(self) -> Self::Delegate {
        Deferred::value(self)
    }
}

/// Copies of the node drawn in one pass, each over the copies before it. Copy `i`, counting
/// from 0, is the node with `i` times each offset added to its own rotation, scale and opacity,
/// so it turns and scales about its own anchor. Its position moves by `position_offset` per
/// column and `row_offset` per row, in canvas pixels. Each offset is evaluated once per frame,
/// not once per copy. Its fields take literals or expressions, but the whole repeat cannot be
/// an expression.
#[derive(Debug, Clone, PartialEq, lumen_macros::Delegate)]
pub struct Repeat {
    /// Copies to draw, from 0 to 1024. 0 draws nothing, and 1 draws the node alone.
    #[meta(min = 0, enforced_min, max = 1024, step = 1, required)]
    pub count: RepeatCount,
    /// Copies per row, after which the next row starts. 0 puts every copy in one row.
    #[meta(min = 0, step = 1)]
    pub columns: u32,
    /// Position in pixels added for each column.
    #[meta()]
    pub position_offset: (f64, f64),
    /// Position in pixels added for each row.
    #[meta()]
    pub row_offset: (f64, f64),
    /// Clockwise rotation in degrees added for each copy.
    #[meta(step = 1)]
    pub rotation_offset: f64,
    /// Scale added for each copy, per axis. A negative offset shrinks later copies through 0,
    /// where they draw nothing, into mirrored copies.
    #[meta(step = 0.01)]
    pub scale_offset: (f64, f64),
    /// Opacity added for each copy. Each copy's opacity is clamped to 0–1.
    #[meta(min = -1, max = 1, step = 0.01)]
    pub opacity_offset: f64,
}

impl Default for Repeat {
    fn default() -> Self {
        Self {
            count: RepeatCount(1),
            columns: 0,
            position_offset: (0.0, 0.0),
            row_offset: (0.0, 0.0),
            rotation_offset: 0.0,
            scale_offset: (0.0, 0.0),
            opacity_offset: 0.0,
        }
    }
}

impl Repeat {
    /// Whether it draws anything other than the node alone.
    pub(crate) fn draws_copies(&self) -> bool {
        self.count.0 != 1
    }

    /// The transform of copy `index` of a node drawn under `base`.
    fn copy(&self, base: &DrawableTransform, index: u32) -> DrawableTransform {
        let (column, row) = match self.columns {
            0 => (index, 0),
            columns => (index % columns, index / columns),
        };
        let [index, column, row] = [index, column, row].map(f64::from);
        let offset = |base: f64, column_step: f64, row_step: f64| {
            base + column * column_step + row * row_step
        };
        DrawableTransform {
            position: (
                offset(base.position.0, self.position_offset.0, self.row_offset.0),
                offset(base.position.1, self.position_offset.1, self.row_offset.1),
            ),
            scale: (
                base.scale.0 + index * self.scale_offset.0,
                base.scale.1 + index * self.scale_offset.1,
            ),
            rotation: base.rotation + index * self.rotation_offset,
            anchor: base.anchor,
            opacity: base.opacity + index * self.opacity_offset,
        }
    }
}

impl RepeatDelegate {
    /// The offsets at either end of the shutter around `frame`, which gives the count and
    /// columns. `None` when motion blur is off or no offset moves.
    pub(crate) fn shutter(
        &self,
        frame: &Repeat,
        motion_blur: f64,
        node_id: NodeId,
        expr: &ExpressionContext<'_>,
    ) -> crate::Result<Option<Shutter<Repeat>>> {
        let Some(offset) = motion::shutter_offset(motion_blur) else {
            return Ok(None);
        };
        if !self.has_expressions() {
            return Ok(None);
        }
        let at = |subframe: f64| -> crate::Result<Repeat> {
            let expr = expr.at_subframe(subframe);
            let evaluated = self
                .eval(&DelegateEvalContext {
                    node_id,
                    expr: &expr,
                })
                .map_err(|error| error.in_field("repeat"))?;
            Ok(Repeat {
                count: frame.count,
                columns: frame.columns,
                ..evaluated
            })
        };
        let (open, close) = (at(-offset)?, at(offset)?);
        Ok((open != *frame || close != *frame).then(|| Shutter {
            open,
            frame: frame.clone(),
            close,
        }))
    }
}

/// A repeated drawable's transform and repeat at the frame, and at either end of the shutter
/// for those that move.
pub(crate) struct RepeatedTransform<'a> {
    pub(crate) base: &'a DrawableTransform,
    pub(crate) base_shutter: Option<&'a Shutter<DrawableTransform>>,
    pub(crate) repeat: &'a Repeat,
    pub(crate) repeat_shutter: Option<&'a Shutter<Repeat>>,
}

impl RepeatedTransform<'_> {
    /// Copy `index` at the frame, and its shutter when it moves. A copy's fields are linear in
    /// those of the base and the offsets, so it moves linearly between the ends too.
    fn copy(&self, index: u32) -> (DrawableTransform, Option<Shutter<DrawableTransform>>) {
        let frame = self.repeat.copy(self.base, index);
        if self.base_shutter.is_none() && self.repeat_shutter.is_none() {
            return (frame, None);
        }
        let end = |base: Option<&DrawableTransform>, repeat: Option<&Repeat>| {
            repeat
                .unwrap_or(self.repeat)
                .copy(base.unwrap_or(self.base), index)
        };
        let shutter = Shutter {
            open: end(
                self.base_shutter.map(|shutter| &shutter.open),
                self.repeat_shutter.map(|shutter| &shutter.open),
            ),
            frame,
            close: end(
                self.base_shutter.map(|shutter| &shutter.close),
                self.repeat_shutter.map(|shutter| &shutter.close),
            ),
        };
        (frame, (!shutter.is_static()).then_some(shutter))
    }

    /// The largest absolute scale components that any copy has at the frame or either end of
    /// its shutter. A copy's scale is linear in its index, so the first and last copies hold
    /// them.
    pub(crate) fn peak_scale(&self) -> (f64, f64) {
        let last = self.repeat.count.0.saturating_sub(1);
        [0, last]
            .into_iter()
            .flat_map(|index| {
                let (frame, shutter) = self.copy(index);
                let ends = shutter.map(|shutter| [shutter.open, shutter.close]);
                std::iter::once(frame).chain(ends.into_iter().flatten())
            })
            .fold((0.0, 0.0), |peak, copy| {
                (
                    peak.0.max(copy.scale.0.abs()),
                    peak.1.max(copy.scale.1.abs()),
                )
            })
    }

    /// The copies that draw anything, for geometry within the untransformed box from `min` to
    /// `max` laid out at the base's position.
    pub(crate) fn copies(&self, min: [f32; 2], max: [f32; 2]) -> DrawableCopies {
        let layout_position = self.base.position;
        let copies = (0..self.repeat.count.0)
            .filter_map(|index| {
                let (frame, shutter) = self.copy(index);
                let motion =
                    DrawableMotion::laid_out_at(shutter.as_ref(), layout_position, min, max);
                motion.is_visible(&frame).then(|| DrawnCopy {
                    gpu: frame.to_gpu_from(layout_position),
                    frame,
                    motion,
                })
            })
            .collect();
        DrawableCopies { copies }
    }
}

struct DrawnCopy {
    frame: DrawableTransform,
    gpu: GpuDrawableTransform,
    motion: DrawableMotion,
}

/// The copies of a repeated drawable that draw anything this frame, in index order. Copies
/// that are transparent or collapsed under every transform they are drawn under are left out,
/// from the bounds and from the draw alike.
pub(crate) struct DrawableCopies {
    copies: Vec<DrawnCopy>,
}

impl DrawableCopies {
    /// The smallest absolute scale component of any copy, over the transforms that draw it.
    pub(crate) fn smallest_scale(&self) -> f64 {
        self.copies
            .iter()
            .map(|copy| copy.motion.smallest_scale(&copy.frame))
            .fold(f64::MAX, f64::min)
    }

    /// The copies, the taps of those that move, and the union of their bounds, for geometry
    /// within the untransformed box from `min` to `max`. Each copy's canvas bounds are grown
    /// by `pad` pixels. Nothing is drawn outside a copy's bounds.
    pub(crate) fn to_gpu(&self, min: [f32; 2], max: [f32; 2], pad: f32) -> GpuCopies {
        let mut gpu = GpuCopies {
            copies: Vec::with_capacity(self.copies.len()),
            taps: Vec::new(),
            bounds: [f32::MAX, f32::MAX, f32::MIN, f32::MIN],
        };
        for copy in &self.copies {
            let [x0, y0, x1, y1] = copy.motion.canvas_bounds(copy.gpu, min, max);
            let bounds = [x0 - pad, y0 - pad, x1 + pad, y1 + pad];
            let first_tap = gpu.taps.len() as u32;
            gpu.taps.extend(copy.motion.gpu_taps());
            gpu.copies.push(GpuRepeatCopy {
                transform: copy.gpu,
                bounds,
                first_tap,
                tap_count: gpu.taps.len() as u32 - first_tap,
                _pad: [0; 2],
            });
            gpu.bounds = union_bounds(gpu.bounds, bounds);
        }
        gpu
    }
}

/// How a drawable reaches the canvas this frame.
pub(crate) enum Drawn<'a> {
    /// Once, under its own transform and motion.
    Once(&'a DrawableTransform, &'a DrawableMotion),
    /// As the copies of its repeat.
    Copies(&'a DrawableCopies),
}

impl Drawn<'_> {
    /// Whether anything is drawn.
    pub(crate) fn is_visible(&self) -> bool {
        match self {
            Self::Once(transform, motion) => motion.is_visible(transform),
            Self::Copies(copies) => !copies.copies.is_empty(),
        }
    }

    /// The smallest absolute scale component over every transform it is drawn under.
    pub(crate) fn smallest_scale(&self) -> f64 {
        match self {
            Self::Once(transform, motion) => motion.smallest_scale(transform),
            Self::Copies(copies) => copies.smallest_scale(),
        }
    }

    /// The canvas bounds of the untransformed box from `min` to `max`, of which `transform` is
    /// the GPU form of the transform drawn once.
    pub(crate) fn canvas_bounds(
        &self,
        transform: GpuDrawableTransform,
        min: [f32; 2],
        max: [f32; 2],
    ) -> [f32; 4] {
        match self {
            Self::Once(_, motion) => motion.canvas_bounds(transform, min, max),
            Self::Copies(copies) => copies.to_gpu(min, max, 0.0).bounds,
        }
    }
}

/// What the repeat entry points of the shape and path shaders draw.
pub(crate) struct GpuCopies {
    pub(crate) copies: Vec<GpuRepeatCopy>,
    pub(crate) taps: Vec<GpuDrawableTransform>,
    /// Canvas-space `[min_x, min_y, max_x, max_y]` around every copy.
    pub(crate) bounds: [f32; 4],
}

impl GpuCopies {
    /// Writes the copies, their taps and their tiles for a repeat entry point drawing into a
    /// texture of `domain`.
    pub(crate) fn storage(
        &self,
        cx: &mut crate::gpu::frame_graph::FrameCx<'_>,
        domain: lumen_gpu::TextureDomain,
    ) -> [lumen_gpu::Res; 3] {
        let size = [domain.storage_size.width, domain.storage_size.height];
        let tiles = self.tiles(
            [domain.data_rect.x, domain.data_rect.y],
            size,
            cx.single_tile_repeats,
        );
        [
            bytemuck::cast_slice(&self.copies),
            bytemuck::cast_slice(&self.taps),
            bytemuck::cast_slice(&tiles),
        ]
        .map(|data| cx.rec.storage_parts(&[data]))
    }

    /// The copies that each tile of a texture `size` texels across may draw, where its first
    /// texel is canvas pixel `origin`.
    ///
    /// The buffer starts with the tile side in texels and the number of tile columns, followed
    /// by a `(start, count)` pair per tile, row by row, and then the copy indices that each tile
    /// lists from `start`, in index order. A tile lists every copy whose bounds may hold one of
    /// its texel centres. Tiles start [`MIN_TILE_SIZE`] texels across and double until the tiles
    /// and their indices fit their caps, which one tile always does. `single_tile` starts with
    /// one tile.
    pub(crate) fn tiles(&self, origin: [i32; 2], size: [u32; 2], single_tile: bool) -> Vec<u32> {
        let spans: Vec<Option<[u32; 4]>> = self
            .copies
            .iter()
            .map(|copy| texel_span(copy.bounds, origin, size))
            .collect();
        let mut tile_size = if single_tile {
            size[0].max(size[1]).max(1)
        } else {
            MIN_TILE_SIZE
        };
        loop {
            let tiles_over = |span: &[u32; 4]| {
                let [x0, y0, x1, y1] = span.map(|texel| u64::from(texel / tile_size));
                (x1 - x0 + 1) * (y1 - y0 + 1)
            };
            let [columns, rows] = size.map(|side| side.div_ceil(tile_size));
            let entries: u64 = spans.iter().flatten().map(tiles_over).sum();
            if u64::from(columns) * u64::from(rows) <= MAX_TILES && entries <= MAX_TILE_ENTRIES {
                return bin_spans(&spans, tile_size, columns, rows, entries as usize);
            }
            tile_size *= 2;
        }
    }
}

/// The inclusive `[x0, y0, x1, y1]` texels whose centres may lie within canvas `bounds`, for a
/// texture `size` texels across whose first texel is canvas pixel `origin`, or `None` when
/// none do. It reaches a texel further each way than the exact span, against f32 rounding in
/// the shader. NaN bounds hold every texel, as the shader's bounds test passes them.
fn texel_span(bounds: [f32; 4], origin: [i32; 2], size: [u32; 2]) -> Option<[u32; 4]> {
    if size.contains(&0) {
        return None;
    }
    let axis = |min: f32, max: f32, origin: i32, size: u32| {
        let start = f64::from(origin) + 0.5;
        let first = (f64::from(min) - start).ceil() - 1.0;
        let last = (f64::from(max) - start).floor() + 1.0;
        let first = first.max(0.0);
        let last = last.min(f64::from(size - 1));
        (first <= last).then_some([first as u32, last as u32])
    };
    let [x0, x1] = axis(bounds[0], bounds[2], origin[0], size[0])?;
    let [y0, y1] = axis(bounds[1], bounds[3], origin[1], size[1])?;
    Some([x0, y0, x1, y1])
}

/// The tile buffer described at [`GpuCopies::tiles`], for copies covering `spans` of texels.
fn bin_spans(
    spans: &[Option<[u32; 4]>],
    tile_size: u32,
    columns: u32,
    rows: u32,
    entries: usize,
) -> Vec<u32> {
    let tile_count = (columns * rows) as usize;
    let tiles_of = |[x0, y0, x1, y1]: [u32; 4]| {
        (y0 / tile_size..=y1 / tile_size).flat_map(move |row| {
            (x0 / tile_size..=x1 / tile_size).map(move |column| (row * columns + column) as usize)
        })
    };
    let mut counts = vec![0u32; tile_count];
    for &span in spans.iter().flatten() {
        for tile in tiles_of(span) {
            counts[tile] += 1;
        }
    }
    let header = 2 + 2 * tile_count;
    let mut buffer = vec![0; header + entries];
    buffer[0] = tile_size;
    buffer[1] = columns;
    let mut start = header as u32;
    for (tile, count) in counts.iter().enumerate() {
        buffer[2 + 2 * tile] = start;
        start += count;
    }
    for (index, span) in spans.iter().enumerate() {
        let Some(span) = *span else { continue };
        for tile in tiles_of(span) {
            let [start, filled] = [buffer[2 + 2 * tile], buffer[3 + 2 * tile]];
            buffer[(start + filled) as usize] = index as u32;
            buffer[3 + 2 * tile] += 1;
        }
    }
    buffer
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub(crate) struct GpuRepeatCopy {
    /// The copy's transform at the frame, drawn when it has no taps.
    pub(crate) transform: GpuDrawableTransform,
    /// Canvas-space `[min_x, min_y, max_x, max_y]` outside which the copy draws nothing.
    pub(crate) bounds: [f32; 4],
    /// The copy's taps in the taps buffer, averaged over when it moves.
    pub(crate) first_tap: u32,
    pub(crate) tap_count: u32,
    pub(crate) _pad: [u32; 2],
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> DrawableTransform {
        DrawableTransform {
            position: (10.0, 20.0),
            scale: (1.0, 1.0),
            rotation: 0.0,
            anchor: (5.0, 5.0),
            opacity: 1.0,
        }
    }

    #[test]
    fn copies_step_by_index_and_wrap_into_rows() {
        let repeat = Repeat {
            count: RepeatCount(5),
            columns: 2,
            position_offset: (30.0, 0.0),
            row_offset: (0.0, 40.0),
            rotation_offset: 15.0,
            scale_offset: (0.5, -0.25),
            opacity_offset: -0.5,
        };
        let copy = repeat.copy(&base(), 3);
        assert_eq!(copy.position, (40.0, 60.0));
        assert_eq!(copy.rotation, 45.0);
        assert_eq!(copy.scale, (2.5, 0.25));
        assert_eq!(copy.anchor, (5.0, 5.0));
        assert_eq!(copy.opacity, -0.5);
        assert_eq!(repeat.copy(&base(), 0), base());
    }

    #[test]
    fn hidden_copies_are_left_out_and_bounds_hold_the_rest() {
        let repeat = Repeat {
            count: RepeatCount(4),
            position_offset: (100.0, 0.0),
            opacity_offset: -0.5,
            ..Repeat::default()
        };
        let copies = RepeatedTransform {
            base: &base(),
            base_shutter: None,
            repeat: &repeat,
            repeat_shutter: None,
        }
        .copies([10.0, 20.0], [20.0, 30.0]);
        let gpu = copies.to_gpu([10.0, 20.0], [20.0, 30.0], 0.0);
        // Opacities 1, 0.5, 0 and -0.5: the last two draw nothing.
        assert_eq!(gpu.copies.len(), 2);
        assert_eq!(gpu.copies[1].transform.opacity, 0.5);
        let [x0, y0, x1, y1] = gpu.bounds;
        assert!(
            (x0 - 10.0).abs() < 0.01 && (x1 - 120.0).abs() < 0.01,
            "{x0} {x1}"
        );
        assert!(
            (y0 - 20.0).abs() < 0.01 && (y1 - 30.0).abs() < 0.01,
            "{y0} {y1}"
        );
    }

    fn copies_within(bounds: &[[f32; 4]]) -> GpuCopies {
        GpuCopies {
            copies: bounds
                .iter()
                .map(|&bounds| GpuRepeatCopy {
                    bounds,
                    ..GpuRepeatCopy::zeroed()
                })
                .collect(),
            taps: Vec::new(),
            bounds: [0.0; 4],
        }
    }

    /// The copies that the tile at `column` and `row` of `tiles` lists.
    fn listed(tiles: &[u32], column: u32, row: u32) -> &[u32] {
        let entry = (2 + 2 * (row * tiles[1] + column)) as usize;
        let start = tiles[entry] as usize;
        &tiles[start..start + tiles[entry + 1] as usize]
    }

    #[test]
    fn tiles_list_the_copies_reaching_them_in_index_order() {
        // The second copy straddles the border between the second and third tile columns.
        let copies = copies_within(&[
            [100.0, 50.0, 110.0, 60.0],
            [120.0, 50.0, 140.0, 60.0],
            [100.0, 50.0, 164.0, 82.0],
        ]);
        let tiles = copies.tiles([100, 50], [64, 32], false);
        assert_eq!(tiles[..2], [MIN_TILE_SIZE, 4]);
        assert_eq!(listed(&tiles, 0, 0), [0, 2]);
        assert_eq!(listed(&tiles, 1, 0), [1, 2]);
        assert_eq!(listed(&tiles, 2, 0), [1, 2]);
        assert_eq!(listed(&tiles, 3, 0), [2]);
        assert_eq!(listed(&tiles, 0, 1), [2]);
        let single = copies.tiles([100, 50], [64, 32], true);
        assert_eq!(single[..2], [64, 1]);
        assert_eq!(listed(&single, 0, 0), [0, 1, 2]);
    }

    #[test]
    fn tiles_coarsen_until_every_copy_fits() {
        let copies = copies_within(&vec![[0.0, 0.0, 1920.0, 1080.0]; MAX_REPEAT_COUNT as usize]);
        let tiles = copies.tiles([0, 0], [1920, 1080], false);
        let [tile_size, columns] = [tiles[0], tiles[1]];
        let tile_count = columns * 1080u32.div_ceil(tile_size);
        assert!(u64::from(tile_count) <= MAX_TILES);
        assert!((tiles.len() - 2 - 2 * tile_count as usize) as u64 <= MAX_TILE_ENTRIES);
        let every: Vec<u32> = (0..MAX_REPEAT_COUNT).collect();
        assert_eq!(listed(&tiles, columns - 1, 0), every);
    }

    #[test]
    fn counts_beyond_the_limit_are_refused() {
        assert_eq!(
            RepeatCount::from_json_value(&Value::from(1024), "count"),
            Ok(RepeatCount(1024))
        );
        for count in [-1, 1025] {
            assert!(RepeatCount::from_json_value(&Value::from(count), "count").is_err());
        }
        assert!(RepeatCount::from_expression(ExpressionValue::Number(2000.0)).is_err());
    }
}
