use std::ops::Range;

use super::path::{POINT_CLOSED, POINT_END, POINT_FILL_HIDDEN};
use super::renderer::{PathPoint, PathUniforms, STYLE_FILL_ENABLED, STYLE_STROKE_ENABLED};
use super::repeat::Drawn;

/// Most bands a path is split into.
const MAX_PATH_BANDS: usize = 1024;
/// Most times a path's grid halves its cells along either side.
const MAX_GRID_LEVEL: u32 = 5;
/// Smallest side of a grid cell, in canvas pixels.
const MIN_CELL_PIXELS: f64 = 16.0;
/// Band and cell entries reserved per point. Paths that would list more get fewer, taller bands
/// and larger cells.
const BAND_ENTRIES_PER_POINT: usize = 8;

// Bits of a cell's count; keep in sync with `path_renderer.wgsl`. The fill covers all or none of
// the cell, as `CELL_INSIDE` says, so the shader neither counts crossings nor measures fill edges.
const CELL_FILL_DECIDED: u32 = 1 << 31;
const CELL_INSIDE: u32 = 1 << 30;

/// Length in `u32`s of the bands buffer for a path of up to `point_capacity` points.
pub(super) fn band_capacity(point_capacity: usize) -> usize {
    2 * MAX_PATH_BANDS
        + 2 * (1 << (2 * MAX_GRID_LEVEL))
        + BAND_ENTRIES_PER_POINT * point_capacity.max(1)
}

/// Splits the path into horizontal bands and a grid of cells in untransformed space, and fills
/// in the culling fields of `uniforms`.
///
/// A band lists the segments whose rows it holds, which are all that a pixel's horizontal
/// crossing ray can meet, so the fill rule reads only its band. A cell lists the segments that
/// can decide the coverage of one of its pixels: those near enough to change the stroke or the
/// fill's edge, and of the fill's edges only those that can be the nearest one. Where the fill
/// covers all or none of a cell the cell says so instead, and lists only what the stroke needs.
///
/// The buffer starts with a `(start, count)` pair per band, then one per cell from
/// `grid_start`, then the index of the first point of each listed segment. An empty buffer means
/// nothing is drawn. Everything is measured for every transform the path is `drawn` under, and
/// the grid only covers what those map a canvas of `canvas` pixels back to. The screen bounds
/// are set for a path drawn once, and left empty for copies, whose bounds are their own. `min`
/// and `max` are the box around `points`.
pub(super) fn bin_path(
    uniforms: &mut PathUniforms,
    points: &[PathPoint],
    (min, max): ([f32; 2], [f32; 2]),
    drawn: &Drawn<'_>,
    canvas: [f32; 2],
    capacity: usize,
) -> Vec<u32> {
    uniforms.screen_bounds = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
    uniforms.band_count = 0;
    uniforms.grid_size = [0, 0];
    // Untransformed size of a canvas pixel, at its largest.
    let pixel = 1.0 / drawn.smallest_scale();
    let offset = f64::from(uniforms.fill_offset);
    let half_feather = f64::from(uniforms.fill_feather) * 0.5;
    // No point inside the fill is deeper than half the box's smaller side, so a fill shrunk
    // by more, past its fade and a pixel, covers nothing.
    let max_depth = f64::from((max[0] - min[0]).min(max[1] - min[1])) * 0.5;
    if offset + half_feather + pixel <= -max_depth {
        uniforms.flags &= !STYLE_FILL_ENABLED;
    }
    let fill = uniforms.flags & STYLE_FILL_ENABLED != 0;
    let stroke = uniforms.flags & STYLE_STROKE_ENABLED != 0;
    let segments = path_segments(points, fill, stroke);
    if segments.is_empty() || !(fill || stroke) || !drawn.is_visible() {
        return Vec::new();
    }

    // The depth inside the outline within which the fill's coverage still varies.
    let fill_depth = if fill {
        (half_feather - offset).min(max_depth)
    } else {
        f64::NEG_INFINITY
    };
    let extent = segment_extent(uniforms, drawn.smallest_scale());
    let reach = extent.max(pixel + fill_depth);
    let largest = [min, max]
        .iter()
        .flatten()
        .fold(0.0, |largest: f64, value| {
            largest.max(f64::from(*value).abs())
        });
    // Covers rounding differences between this binning and the shader's lookups and measures.
    let margin = 1e-4 * (1.0 + largest + reach);
    let (min, max) = (min.map(f64::from), max.map(f64::from));

    let mut data = Vec::new();
    let bands = if fill {
        let budget = capacity.saturating_sub(2 + segments.len());
        Bands::new(
            &segments,
            [min[1], max[1]],
            pixel,
            margin,
            budget,
            &mut data,
        )
    } else {
        Bands::default()
    };
    let soft = offset != 0.0 || half_feather > 0.0;
    let measure = Measure {
        segments: &segments,
        points,
        bands: &bands,
        data: &data,
        margin,
        fill,
        soft,
        uniform: drawn.has_uniform_scale(),
        reach,
        fill_reach: if soft {
            [pixel + fill_depth, pixel + offset + half_feather]
        } else {
            [pixel, pixel]
        },
        stroke_reach: if stroke {
            f64::from(uniforms.stroke_width).max(0.0) * 0.5 + pixel
        } else {
            f64::NEG_INFINITY
        },
        covered_within: if soft {
            offset - half_feather - pixel - margin
        } else {
            f64::NEG_INFINITY
        },
        empty_within: if soft {
            -offset - half_feather - pixel - margin
        } else {
            f64::NEG_INFINITY
        },
    };
    let pad = reach + margin;
    let mut grid = Rect {
        min: [min[0] - pad, min[1] - pad],
        max: [max[0] + pad, max[1] + pad],
    };
    // Every pixel the shader draws lies on the canvas.
    if let Some((low, high)) = drawn.local_bounds(canvas) {
        let largest = low
            .iter()
            .chain(&high)
            .fold(0.0, |largest: f64, value| largest.max(value.abs()));
        let slack = margin + 1e-4 * largest;
        let position = uniforms.position.map(f64::from);
        for axis in 0..2 {
            grid.min[axis] = grid.min[axis].max(low[axis] - position[axis] - slack);
            grid.max[axis] = grid.max[axis].min(high[axis] - position[axis] + slack);
        }
        if grid.min[0] >= grid.max[0] || grid.min[1] >= grid.max[1] {
            return Vec::new();
        }
    }
    let grid_start = data.len();
    let (size, cells) = measure.grid(
        &grid,
        pixel,
        grid_start,
        capacity.saturating_sub(grid_start),
    );
    data.extend(cells);

    if let Drawn::Once(_, motion) = drawn {
        let extent = extent as f32;
        let [x, y] = uniforms.position;
        let (min, max) = (min.map(|value| value as f32), max.map(|value| value as f32));
        uniforms.screen_bounds = motion.canvas_bounds(
            uniforms.transform,
            [min[0] - extent + x, min[1] - extent + y],
            [max[0] + extent + x, max[1] + extent + y],
        );
    }
    uniforms.band_origin = bands.origin as f32;
    uniforms.band_scale = if bands.count > 0 {
        (bands.count as f64 / bands.height) as f32
    } else {
        0.0
    };
    uniforms.band_count = bands.count as u32;
    uniforms.grid_origin = grid.min.map(|value| value as f32);
    uniforms.grid_scale =
        [0, 1].map(|axis| (f64::from(size[axis]) / (grid.max[axis] - grid.min[axis])) as f32);
    uniforms.grid_size = size;
    uniforms.grid_start = grid_start as u32;
    uniforms.reach = reach as f32;
    data
}

/// Untransformed distance past the outline within which a segment can still cover part of a
/// pixel when drawn at a smallest absolute scale component of `scale`: a canvas pixel of
/// antialiasing past the farther of the stroke's half width and the fill's offset edge and fade,
/// or just the pixel when neither is drawn.
pub(super) fn segment_extent(uniforms: &PathUniforms, scale: f64) -> f64 {
    let stroke = if uniforms.flags & STYLE_STROKE_ENABLED != 0 {
        f64::from(uniforms.stroke_width).max(0.0) * 0.5
    } else {
        f64::NEG_INFINITY
    };
    let fill = if uniforms.flags & STYLE_FILL_ENABLED != 0 {
        f64::from(uniforms.fill_offset) + f64::from(uniforms.fill_feather) * 0.5
    } else {
        f64::NEG_INFINITY
    };
    let drawn = stroke.max(fill);
    1.0 / scale + if drawn.is_finite() { drawn } else { 0.0 }
}

// What the shader reads a segment for.
const CROSSES: u8 = 1;
const FILL_EDGE: u8 = 2;
const STROKED: u8 = 4;
// Which corners of its box the segment runs between, and whether it is so nearly horizontal that
// the shader's crossing test can place it anywhere within its width beyond its box, as it does not
// divide by its height.
const ENDS_SWAP_X: u8 = 8;
const ENDS_SWAP_Y: u8 = 16;
const NEARLY_LEVEL: u8 = 32;

/// A segment, laid out in one cache line for the bounds measured against every cell.
struct Segment {
    min: [f64; 2],
    max: [f64; 2],
    /// Unit normal, or zero for a segment of no length.
    normal: [f64; 2],
    /// `normal` dotted with either end.
    offset: f64,
    point: u32,
    roles: u8,
}

/// The segments that the shader reads, each from a point to the next one. The open end of a
/// contour joins its start for the fill only, never as a stroke.
fn path_segments(points: &[PathPoint], fill: bool, stroke: bool) -> Vec<Segment> {
    points
        .iter()
        .enumerate()
        .filter_map(|(index, point)| {
            let next = next_point(points, index);
            let open_end = point.flags & POINT_END != 0 && point.flags & POINT_CLOSED == 0;
            let mut roles = 0;
            if fill {
                roles |= CROSSES;
                if point.flags & POINT_FILL_HIDDEN == 0 {
                    roles |= FILL_EDGE;
                }
            }
            if stroke && !open_end {
                roles |= STROKED;
            }
            if next == index || roles == 0 {
                return None;
            }
            let a = point.position.map(f64::from);
            let b = points[next].position.map(f64::from);
            if a[0] > b[0] {
                roles |= ENDS_SWAP_X;
            }
            if a[1] > b[1] {
                roles |= ENDS_SWAP_Y;
            }
            if (points[next].position[1] - point.position[1]).abs() < 2e-6 {
                roles |= NEARLY_LEVEL;
            }
            let length = (b[0] - a[0]).hypot(b[1] - a[1]);
            let normal = if length > 0.0 {
                [(a[1] - b[1]) / length, (b[0] - a[0]) / length]
            } else {
                [0.0, 0.0]
            };
            Some(Segment {
                min: [a[0].min(b[0]), a[1].min(b[1])],
                max: [a[0].max(b[0]), a[1].max(b[1])],
                normal,
                offset: normal[0] * a[0] + normal[1] * a[1],
                point: index as u32,
                roles,
            })
        })
        .collect()
}

fn next_point(points: &[PathPoint], index: usize) -> usize {
    let point = &points[index];
    if point.flags & POINT_END != 0 {
        point.contour_start as usize
    } else {
        index + 1
    }
}

#[derive(Clone, Copy)]
struct Rect {
    min: [f64; 2],
    max: [f64; 2],
}

impl Rect {
    fn centre(&self) -> [f64; 2] {
        [0, 1].map(|axis| (self.min[axis] + self.max[axis]) * 0.5)
    }

    /// The `columns` by `rows` cell at `[x, y]` of an even split of the rect.
    fn cell(&self, [columns, rows]: [u32; 2], [x, y]: [u32; 2]) -> Self {
        let at = |axis: usize, count: u32, index: u32| {
            self.min[axis] + (self.max[axis] - self.min[axis]) * f64::from(index) / f64::from(count)
        };
        Self {
            min: [at(0, columns, x), at(1, rows, y)],
            max: [at(0, columns, x + 1), at(1, rows, y + 1)],
        }
    }
}

fn point_segment_distance(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let ab = [b[0] - a[0], b[1] - a[1]];
    let ap = [p[0] - a[0], p[1] - a[1]];
    let length_squared = ab[0] * ab[0] + ab[1] * ab[1];
    let t = if length_squared > 0.0 {
        ((ap[0] * ab[0] + ap[1] * ab[1]) / length_squared).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let [x, y] = [ap[0] - ab[0] * t, ap[1] - ab[1] * t];
    (x * x + y * y).sqrt()
}

impl Segment {
    fn ends(&self) -> [[f64; 2]; 2] {
        let [x0, x1] = if self.roles & ENDS_SWAP_X != 0 {
            [self.max[0], self.min[0]]
        } else {
            [self.min[0], self.max[0]]
        };
        let [y0, y1] = if self.roles & ENDS_SWAP_Y != 0 {
            [self.max[1], self.min[1]]
        } else {
            [self.min[1], self.max[1]]
        };
        [[x0, y0], [x1, y1]]
    }

    /// The gap along each axis between the segment's box, widened by `slack` along x, and `rect`.
    fn gaps(&self, rect: &Rect, slack: f64) -> [f64; 2] {
        // Plain comparisons, as nothing here is NaN.
        let gap = |axis: usize, slack: f64| {
            let before = self.min[axis] - slack - rect.max[axis];
            let after = rect.min[axis] - self.max[axis] - slack;
            let gap = if before > after { before } else { after };
            if gap > 0.0 { gap } else { 0.0 }
        };
        [gap(0, slack), gap(1, 0.0)]
    }

    /// A lower bound on the squared distance from `rect`, centred on `centre` and `half` its size,
    /// to the segment: the larger of the distances to its box and to its line.
    fn lower_squared(&self, rect: &Rect, centre: [f64; 2], half: [f64; 2]) -> f64 {
        let [x, y] = self.gaps(rect, 0.0);
        let line = (self.normal[0] * centre[0] + self.normal[1] * centre[1] - self.offset).abs()
            - (self.normal[0].abs() * half[0] + self.normal[1].abs() * half[1]);
        let boxed = x * x + y * y;
        let line = if line > 0.0 { line * line } else { 0.0 };
        if boxed > line { boxed } else { line }
    }

    /// The largest distance from a point of `rect` to the segment, which lies at a corner as the
    /// distance is convex.
    fn upper(&self, rect: &Rect) -> f64 {
        let [a, b] = self.ends();
        [
            rect.min,
            [rect.max[0], rect.min[1]],
            [rect.min[0], rect.max[1]],
            rect.max,
        ]
        .into_iter()
        .map(|corner| point_segment_distance(corner, a, b))
        .fold(0.0, f64::max)
    }

    /// A lower bound on the squared distance from `rect` to wherever the shader's crossing test
    /// can place the segment, given the one to the segment itself.
    fn crossing_lower_squared(&self, rect: &Rect, lower_squared: f64) -> f64 {
        if self.roles & NEARLY_LEVEL == 0 {
            return lower_squared;
        }
        let [x, y] = self.gaps(rect, self.max[0] - self.min[0]);
        x * x + y * y
    }
}

/// Horizontal bands over the outline's rows, listing every segment that crosses one of their
/// rows, padded only against rounding.
#[derive(Default)]
struct Bands {
    origin: f64,
    height: f64,
    count: usize,
}

impl Bands {
    /// Bins `segments` into `data` in bands about a pixel tall over the rows from `min` to `max`,
    /// fewer where they would not fit in `budget` entries.
    fn new(
        segments: &[Segment],
        [min, max]: [f64; 2],
        pixel: f64,
        pad: f64,
        budget: usize,
        data: &mut Vec<u32>,
    ) -> Self {
        let origin = min - pad;
        let height = max + pad - origin;
        let mut bands = Self {
            origin,
            height,
            count: (height / pixel)
                .floor()
                .clamp(1.0, segments.len().min(MAX_PATH_BANDS) as f64) as usize,
        };
        let range = |bands: &Self, segment: &Segment| {
            bands.band(segment.min[1] - pad)..=bands.band(segment.max[1] + pad)
        };
        while bands.count > 1
            && 2 * bands.count
                + segments
                    .iter()
                    .map(|segment| range(&bands, segment).count())
                    .sum::<usize>()
                > budget
        {
            bands.count /= 2;
        }

        data.resize(2 * bands.count, 0);
        for segment in segments {
            for band in range(&bands, segment) {
                data[2 * band + 1] += 1;
            }
        }
        let mut start = 2 * bands.count as u32;
        for band in 0..bands.count {
            data[2 * band] = start;
            start += data[2 * band + 1];
        }
        data.resize(start as usize, 0);
        let mut cursors: Vec<u32> = (0..bands.count).map(|band| data[2 * band]).collect();
        for segment in segments {
            for band in range(&bands, segment) {
                data[cursors[band] as usize] = segment.point;
                cursors[band] += 1;
            }
        }
        bands
    }

    fn band(&self, y: f64) -> usize {
        ((y - self.origin) * self.count as f64 / self.height).clamp(0.0, (self.count - 1) as f64)
            as usize
    }
}

/// How far each kind of segment reaches, and the data the fill rule is evaluated from.
struct Measure<'a> {
    segments: &'a [Segment],
    points: &'a [PathPoint],
    bands: &'a Bands,
    data: &'a [u32],
    margin: f64,
    fill: bool,
    /// Whether the fill is offset or feathered, so its coverage depends on the nearest edge.
    soft: bool,
    /// Whether every transform scales both axes alike, so the untransformed nearest edge is
    /// also the nearest in canvas pixels.
    uniform: bool,
    /// The shader's `reach`, within which it measures a segment.
    reach: f64,
    /// Untransformed distance from the outline, inside and outside it, beyond which the fill's
    /// coverage is 1 and 0.
    fill_reach: [f64; 2],
    /// Untransformed distance from a stroked segment beyond which it covers nothing.
    stroke_reach: f64,
    /// Where the nearest fill edge is at most this far, the soft fill covers the point fully
    /// whichever side it is on, and where at most `empty_within`, not at all.
    covered_within: f64,
    empty_within: f64,
}

/// Which fill edges a cell lists.
enum FillEdges {
    Unneeded,
    /// Those whose squared distance from the cell is at most this.
    Within(f64),
    /// Every one the shader would measure from some point of the cell.
    InReach,
}

/// A cell of one level of the grid: where it is, its fill flags, and the range of its listed
/// segments in the level's lists.
struct Node {
    cell: [u32; 2],
    fill: u32,
    list: Range<usize>,
}

struct Level {
    size: [u32; 2],
    nodes: Vec<Node>,
    lists: Vec<u32>,
}

impl Level {
    fn entries(&self) -> usize {
        2 * (self.size[0] * self.size[1]) as usize + self.lists.len()
    }
}

impl Measure<'_> {
    /// Splits `grid` into cells, and returns how many it has along each side and their pairs and
    /// lists, to be placed at index `start` of the buffer. Cells are halved level by level,
    /// keeping only segments their parent listed, while the lists fit in `budget` entries and
    /// cells stay `MIN_CELL_PIXELS` wide. A cell that lists nothing is not split further.
    fn grid(&self, grid: &Rect, pixel: f64, start: usize, budget: usize) -> ([u32; 2], Vec<u32>) {
        let side = (grid.max[0] - grid.min[0]).max(grid.max[1] - grid.min[1]);
        let cell = (side / f64::from(1 << MAX_GRID_LEVEL)).max(MIN_CELL_PIXELS * pixel);
        let levels = [0, 1].map(|axis| {
            ((grid.max[axis] - grid.min[axis]) / cell)
                .log2()
                .round()
                .clamp(0.0, f64::from(MAX_GRID_LEVEL)) as u32
        });
        let mut lowers = Vec::new();
        let mut root = Level {
            size: [1, 1],
            nodes: Vec::new(),
            lists: Vec::new(),
        };
        let all: Vec<u32> = (0..self.segments.len() as u32).collect();
        let fill = if self.fill { 0 } else { CELL_FILL_DECIDED };
        let fill = self.refine(grid, fill, &all, &mut root.lists, &mut lowers);
        root.nodes.push(Node {
            cell: [0, 0],
            fill,
            list: 0..root.lists.len(),
        });
        let mut done = vec![root];
        for depth in 1..=levels[0].max(levels[1]) {
            let parent = done.last().expect("the root level is built");
            if parent.lists.is_empty() {
                break;
            }
            let split = levels.map(|level| u32::from(depth <= level));
            let size = [parent.size[0] << split[0], parent.size[1] << split[1]];
            let mut level = Level {
                size,
                nodes: Vec::new(),
                lists: Vec::new(),
            };
            for node in parent.nodes.iter().filter(|node| !node.list.is_empty()) {
                let list = &parent.lists[node.list.clone()];
                for dy in 0..=split[1] {
                    for dx in 0..=split[0] {
                        let cell = [
                            (node.cell[0] << split[0]) + dx,
                            (node.cell[1] << split[1]) + dy,
                        ];
                        let start = level.lists.len();
                        let fill = self.refine(
                            &grid.cell(size, cell),
                            node.fill,
                            list,
                            &mut level.lists,
                            &mut lowers,
                        );
                        level.nodes.push(Node {
                            cell,
                            fill,
                            list: start..level.lists.len(),
                        });
                    }
                }
            }
            if level.entries() > budget {
                break;
            }
            done.push(level);
        }

        // A cell that lists nothing stands for all of its part of the finest level.
        let size = done.last().expect("the root level is built").size;
        let mut data = vec![0; 2 * (size[0] * size[1]) as usize];
        let last = done.len() - 1;
        for (depth, level) in done.iter().enumerate() {
            let scale = [size[0] / level.size[0], size[1] / level.size[1]];
            for node in &level.nodes {
                if depth < last && !node.list.is_empty() {
                    continue;
                }
                let list_start = (start + data.len()) as u32;
                data.extend(
                    level.lists[node.list.clone()]
                        .iter()
                        .map(|&segment| self.segments[segment as usize].point),
                );
                let count = node.list.len() as u32 | node.fill;
                for y in node.cell[1] * scale[1]..(node.cell[1] + 1) * scale[1] {
                    for x in node.cell[0] * scale[0]..(node.cell[0] + 1) * scale[0] {
                        let entry = 2 * (y * size[0] + x) as usize;
                        data[entry] = list_start;
                        data[entry + 1] = count;
                    }
                }
            }
        }
        (size, data)
    }

    /// Appends to `list` the segments of `parent` that can decide a pixel in `rect`, and returns
    /// the cell's fill flags, which start as `fill`.
    ///
    /// The fill rule is constant across a cell that no segment comes near, and is then read at
    /// its centre. A sharp fill's coverage only varies within a pixel of an edge, and a soft
    /// fill's with the distance to the nearest edge. Where that distance alone covers or empties
    /// the whole cell, whichever side of the outline it is on, the cell says so. Otherwise only
    /// the edges that can be the nearest from some point of the cell are listed: those no
    /// farther from the cell than the nearest edge is from its farthest point. Under non-uniform
    /// scale the antialiasing compares canvas distances to every edge within reach, so all of
    /// those are listed.
    fn refine(
        &self,
        rect: &Rect,
        mut fill: u32,
        parent: &[u32],
        list: &mut Vec<u32>,
        lowers: &mut Vec<f64>,
    ) -> u32 {
        // Bounds are compared squared, against squares of distances that are never negative.
        let squared = |distance: f64| {
            if distance < 0.0 {
                -1.0
            } else {
                distance * distance
            }
        };
        let centre = rect.centre();
        let half = [0, 1].map(|axis| (rect.max[axis] - rect.min[axis]) * 0.5);
        let margin = squared(self.margin);
        let near = |segment: &Segment, lower: f64| {
            segment.roles & CROSSES != 0 && segment.crossing_lower_squared(rect, lower) <= margin
        };
        let mut nearest_lower = f64::INFINITY;
        let mut nearest_edge = None;
        let mut near_crossing = false;
        lowers.clear();
        for &index in parent {
            let segment = &self.segments[index as usize];
            let lower = segment.lower_squared(rect, centre, half);
            lowers.push(lower);
            if fill == 0 {
                near_crossing |= near(segment, lower);
                if segment.roles & FILL_EDGE != 0 && lower < nearest_lower {
                    nearest_lower = lower;
                    nearest_edge = Some(segment);
                }
            }
        }
        let mut fill_edges = FillEdges::Unneeded;
        if fill == 0 {
            // How far, at most, the nearest fill edge is from a point of the cell: no farther than
            // the edge nearest the cell is from its farthest point.
            let nearest = match nearest_edge {
                Some(segment) if self.soft => segment.upper(rect),
                _ => f64::INFINITY,
            };
            if nearest <= self.covered_within {
                fill = CELL_FILL_DECIDED | CELL_INSIDE;
            } else if nearest <= self.empty_within {
                fill = CELL_FILL_DECIDED;
            } else {
                let inside = (!near_crossing).then(|| self.inside(centre));
                let reach = match inside {
                    Some(true) => self.fill_reach[0],
                    Some(false) => self.fill_reach[1],
                    None => self.fill_reach[0].max(self.fill_reach[1]),
                };
                fill_edges = match inside {
                    Some(inside) if nearest_lower > squared(reach + self.margin) => {
                        fill = CELL_FILL_DECIDED | if inside { CELL_INSIDE } else { 0 };
                        FillEdges::Unneeded
                    }
                    _ if self.soft && !self.uniform => FillEdges::InReach,
                    _ => FillEdges::Within(squared(reach.min(nearest) + self.margin)),
                };
            }
        }
        let stroke_within = squared(self.stroke_reach + self.margin);
        let reach = self.reach + self.margin;
        list.extend(
            parent
                .iter()
                .zip(lowers.iter())
                .filter(|&(&segment, &lower)| {
                    let segment = &self.segments[segment as usize];
                    let fill_edge = segment.roles & FILL_EDGE != 0
                        && match fill_edges {
                            FillEdges::Unneeded => false,
                            FillEdges::Within(within) => lower <= within,
                            // The shader's own test: within reach of the segment's box on both axes.
                            FillEdges::InReach => {
                                let [x, y] = segment.gaps(rect, 0.0);
                                x.max(y) <= reach
                            }
                        };
                    fill_edge
                        || (segment.roles & STROKED != 0 && lower <= stroke_within)
                        || (fill == 0 && near(segment, lower))
                })
                .map(|(&segment, _)| segment),
        );
        fill
    }

    /// Whether `point` is inside the fill under the shader's crossing rule, from its band.
    fn inside(&self, [x, y]: [f64; 2]) -> bool {
        let band = self.bands.band(y);
        let start = self.data[2 * band] as usize;
        let count = self.data[2 * band + 1] as usize;
        self.data[start..start + count]
            .iter()
            .filter(|&&index| {
                let a = self.points[index as usize].position.map(f64::from);
                let b = self.points[next_point(self.points, index as usize)]
                    .position
                    .map(f64::from);
                (a[1] > y) != (b[1] > y) && x < (b[0] - a[0]) * (y - a[1]) / (b[1] - a[1]) + a[0]
            })
            .count()
            % 2
            == 1
    }
}

#[cfg(test)]
mod tests {
    use super::super::drawable::{DrawableMotion, DrawableTransform};
    use super::super::path::{parse_path_points, point_bounds};
    use super::*;
    use bytemuck::Zeroable;

    fn identity() -> DrawableTransform {
        DrawableTransform {
            position: (0.0, 0.0),
            scale: (1.0, 1.0),
            rotation: 0.0,
            anchor: (0.0, 0.0),
            opacity: 1.0,
        }
    }

    fn points_of(data: &str) -> Vec<PathPoint> {
        parse_path_points(data, 0.25, 4096).unwrap()
    }

    fn bin(data: &str, capacity: usize) -> (PathUniforms, Vec<u32>) {
        bin_soft(data, capacity, 0.0, 0.0)
    }

    fn bin_soft(
        data: &str,
        capacity: usize,
        offset: f32,
        feather: f32,
    ) -> (PathUniforms, Vec<u32>) {
        // A canvas no edge of which clips the grid.
        bin_on(data, capacity, offset, feather, [f32::INFINITY; 2])
    }

    fn bin_on(
        data: &str,
        capacity: usize,
        offset: f32,
        feather: f32,
        canvas: [f32; 2],
    ) -> (PathUniforms, Vec<u32>) {
        let transform = identity();
        let mut uniforms = PathUniforms::zeroed();
        uniforms.flags = STYLE_FILL_ENABLED;
        uniforms.fill_offset = offset;
        uniforms.fill_feather = feather;
        uniforms.transform = transform.to_gpu();
        let points = points_of(data);
        let data = bin_path(
            &mut uniforms,
            &points,
            point_bounds(&points),
            &Drawn::Once(&transform, &DrawableMotion::default()),
            canvas,
            capacity,
        );
        (uniforms, data)
    }

    fn band(data: &[u32], band: usize) -> &[u32] {
        let (start, count) = (data[2 * band] as usize, data[2 * band + 1] as usize);
        &data[start..start + count]
    }

    #[test]
    fn lists_each_segment_in_the_bands_it_reaches() {
        // A tall triangle: its two slanted sides span every band, its base only the last.
        let (uniforms, data) = bin("M 0 0 L 10 100 L -10 100 Z", 4096);
        let count = uniforms.band_count as usize;
        assert!(count > 1);
        assert_eq!(band(&data, 0), &[0, 2]);
        assert_eq!(band(&data, count - 1), &[0, 1, 2]);
        for (bound, expected) in uniforms
            .screen_bounds
            .iter()
            .zip([-11.0, -1.0, 11.0, 101.0])
        {
            assert!(
                (bound - expected).abs() < 0.01,
                "{bound} is not near {expected}"
            );
        }
    }

    #[test]
    fn offset_and_feather_move_the_drawn_bounds() {
        // A 40 by 20 box, drawn a pixel past its fill's faded edge.
        let bounds = |offset: f32, feather: f32| {
            let (uniforms, data) = bin_soft("M 0 0 H 40 V 20 H 0 Z", 4096, offset, feather);
            (!data.is_empty()).then_some(uniforms.screen_bounds)
        };
        let near = |bounds: Option<[f32; 4]>, expected: [f32; 4]| {
            let bounds = bounds.expect("something is drawn");
            assert!(
                bounds
                    .iter()
                    .zip(expected)
                    .all(|(a, b)| (a - b).abs() < 0.01),
                "{bounds:?} is not near {expected:?}"
            );
        };
        near(bounds(0.0, 0.0), [-1.0, -1.0, 41.0, 21.0]);
        near(bounds(6.0, 4.0), [-9.0, -9.0, 49.0, 29.0]);
        near(bounds(-4.0, 0.0), [3.0, 3.0, 37.0, 17.0]);
        // Shrunk by its half height and a pixel, nothing is left to draw.
        assert!(bounds(-10.5, 0.0).is_some());
        assert_eq!(bounds(-11.0, 0.0), None);
        assert!(bounds(-12.5, 4.0).is_some());
        assert_eq!(bounds(-13.0, 4.0), None);
    }

    /// The segments listed in the cell holding `p`, and the cell's fill flags, read as the
    /// shader reads them.
    fn cell(uniforms: &PathUniforms, data: &[u32], p: [f32; 2]) -> (Vec<u32>, u32) {
        let [x, y] = [0, 1].map(|axis| {
            ((p[axis] - uniforms.grid_origin[axis]) * uniforms.grid_scale[axis]) as u32
        });
        let entry = uniforms.grid_start as usize + 2 * (y * uniforms.grid_size[0] + x) as usize;
        let (start, count) = (data[entry] as usize, data[entry + 1]);
        let flags = count & (CELL_FILL_DECIDED | CELL_INSIDE);
        let count = (count & !flags) as usize;
        (data[start..start + count].to_vec(), flags)
    }

    #[test]
    fn coarsens_bands_and_cells_to_fit_capacity() {
        // Three bands would need 13 entries and four cells 8 more; one of each lists every
        // segment once.
        let (uniforms, data) = bin("M 0 0 L 10 100 L -10 100 Z", 12);
        assert_eq!(uniforms.band_count, 1);
        assert_eq!(uniforms.grid_size, [1, 1]);
        assert_eq!(data, [2, 3, 0, 1, 2, 7, 3, 0, 1, 2]);
        let (uniforms, _) = bin("M 0 0 L 10 100 L -10 100 Z", 4096);
        assert!(uniforms.grid_size[1] > 1);
    }

    #[test]
    fn cells_decide_the_fill_away_from_its_edges() {
        let square = "M 0 0 H 1000 V 1000 H 0 Z";
        let (uniforms, data) = bin(square, 4096);
        assert_eq!(
            cell(&uniforms, &data, [500.0, 500.0]),
            (vec![], CELL_FILL_DECIDED | CELL_INSIDE)
        );
        // Beside the top edge only the top edge can change the antialiasing.
        assert_eq!(cell(&uniforms, &data, [500.0, 0.5]), (vec![0], 0));
    }

    #[test]
    fn the_grid_ends_with_the_canvas() {
        let (uniforms, _) = bin_on("M 0 0 H 1000 V 1000 H 0 Z", 4096, 0.0, 0.0, [400.0, 300.0]);
        let [start, end] = [0, 1].map(|end| {
            [0, 1].map(|axis| {
                uniforms.grid_origin[axis]
                    + (end * uniforms.grid_size[axis]) as f32 / uniforms.grid_scale[axis]
            })
        });
        let near = |a: [f32; 2], b: [f32; 2]| a.iter().zip(b).all(|(a, b)| (a - b).abs() < 0.5);
        assert!(
            near(start, [0.0, 0.0]) && near(end, [400.0, 300.0]),
            "{start:?} {end:?}"
        );
    }

    #[test]
    fn soft_fills_list_only_edges_that_can_be_nearest() {
        // Grown by 300, a tall box covers everything within 300 of its edges.
        let (uniforms, data) = bin_soft("M 0 0 H 100 V 1000 H 0 Z", 4096, 300.0, 0.0);
        assert_eq!(
            cell(&uniforms, &data, [-150.0, 500.0]),
            (vec![], CELL_FILL_DECIDED | CELL_INSIDE)
        );
        // At its grown edge the left edge is nearer than every other.
        assert_eq!(cell(&uniforms, &data, [-300.0, 500.0]), (vec![3], 0));
        // The grid reaches past the grown edge, which rounds off its corners.
        assert_eq!(
            cell(&uniforms, &data, [-295.0, -295.0]),
            (vec![], CELL_FILL_DECIDED)
        );
    }

    #[test]
    fn invisible_paths_list_nothing() {
        let transform = DrawableTransform {
            opacity: 0.0,
            ..identity()
        };
        let mut uniforms = PathUniforms::zeroed();
        uniforms.flags = STYLE_FILL_ENABLED;
        uniforms.transform = transform.to_gpu();
        let points = points_of("M 0 0 L 1 0 L 0 1 Z");
        assert!(
            bin_path(
                &mut uniforms,
                &points,
                point_bounds(&points),
                &Drawn::Once(&transform, &DrawableMotion::default()),
                [100.0, 100.0],
                4096
            )
            .is_empty()
        );
        assert_eq!(uniforms.band_count, 0);
        assert_eq!(uniforms.grid_size, [0, 0]);
    }
}
