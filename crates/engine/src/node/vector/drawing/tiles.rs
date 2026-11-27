//! Lists the copies of drawables that can draw in each tile of `render_2d`'s output.

use foldhash::HashMap;

use super::build::{
    Drawing, GpuVectorCopy, GpuVectorDrawable, STYLE_SHADOW, is_opaque, scales, shape_kind,
};
use super::path::STYLE_FILL_ENABLED;
use super::path_bands::{CELL_FILL_DECIDED, CELL_INSIDE};

/// Side in texels of a tile, which shares one list; keep in sync with `drawing.wgsl`.
const TILE_SIZE: u32 = 16;
const CELL_COUNT: u32 = !(CELL_FILL_DECIDED | CELL_INSIDE);
/// Side in tiles of the blocks measured before their tiles.
const BLOCK_TILES: u32 = 4;

/// How much of a tile a copy covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Coverage {
    Nothing,
    Part,
    /// Every pixel, fully.
    All,
}

/// The tile column count and lists of a texture of `size` texels whose first texel is canvas
/// pixel `origin`: a `(start, count)` pair per tile, row by row, then the copies each tile lists,
/// topmost first. A tile leaves out those below an opaque copy that covers all of it.
pub(super) fn tile_lists(drawing: &Drawing, origin: [i32; 2], size: [u32; 2]) -> (u32, Vec<u32>) {
    let [columns, rows] = size.map(|side| side.div_ceil(TILE_SIZE).max(1));
    let tile_count = (columns * rows) as usize;
    let mut hidden = vec![false; tile_count];
    let mut marks = vec![0u8; tile_count];
    let mut marked = Vec::new();
    // The cells of each path drawable's grid, measured under the first of its copies listed.
    let mut cells: HashMap<u32, Vec<Cell>> = HashMap::default();
    // `(tile, copy)` pairs, from the top copy down.
    let mut listed: Vec<(u32, u32)> = Vec::new();
    for index in (0..drawing.copies.len()).rev() {
        let copy = &drawing.copies[index];
        let drawable = &drawing.drawables[copy.drawable as usize];
        let Some(range) = tile_range(copy.screen_bounds, origin, [columns, rows]) else {
            continue;
        };
        let opaque = is_opaque(drawing, copy);
        let mut list = |tile: u32, coverage: Coverage| {
            if coverage == Coverage::Nothing || hidden[tile as usize] {
                return;
            }
            listed.push((tile, index as u32));
            hidden[tile as usize] = opaque && coverage == Coverage::All;
        };
        let Some(ellipse) = shape_kind(drawable) else {
            let cells = cells
                .entry(copy.drawable)
                .or_insert_with(|| path_cells(drawing, drawable, copy));
            mark_path_tiles(
                cells,
                copy,
                origin,
                [columns, rows],
                range,
                &mut marks,
                &mut marked,
            );
            for tile in marked.drain(..) {
                let mark = std::mem::take(&mut marks[tile as usize]);
                let (min, max) = tile_box(origin, columns, tile);
                list(tile, path_coverage((drawable, copy), mark, min, max));
            }
            continue;
        };
        let [x0, y0, x1, y1] = range;
        // Blocks of tiles are measured first, so that tiles the shape misses or covers together
        // are decided at once.
        for by in (y0..=y1).step_by(BLOCK_TILES as usize) {
            for bx in (x0..=x1).step_by(BLOCK_TILES as usize) {
                let last = [
                    (bx + BLOCK_TILES - 1).min(x1),
                    (by + BLOCK_TILES - 1).min(y1),
                ];
                let (min, _) = tile_box(origin, columns, by * columns + bx);
                let (_, max) = tile_box(origin, columns, last[1] * columns + last[0]);
                let block = shape_coverage((drawable, copy), ellipse, min, max);
                if block == Coverage::Nothing {
                    continue;
                }
                for ty in by..=last[1] {
                    for tx in bx..=last[0] {
                        let tile = ty * columns + tx;
                        let coverage = if block == Coverage::All {
                            block
                        } else {
                            let (min, max) = tile_box(origin, columns, tile);
                            shape_coverage((drawable, copy), ellipse, min, max)
                        };
                        list(tile, coverage);
                    }
                }
            }
        }
    }

    let mut data = vec![0; 2 * tile_count];
    for &(tile, _) in &listed {
        data[2 * tile as usize + 1] += 1;
    }
    let mut start = data.len() as u32;
    for tile in 0..tile_count {
        data[2 * tile] = start;
        start += data[2 * tile + 1];
    }
    data.resize(start as usize, 0);
    let mut next: Vec<u32> = (0..tile_count).map(|tile| data[2 * tile]).collect();
    for (tile, copy) in listed {
        data[next[tile as usize] as usize] = copy;
        next[tile as usize] += 1;
    }
    (columns, data)
}

/// The canvas box of tile `tile` of a texture `columns` tiles wide whose first texel is canvas
/// pixel `origin`.
fn tile_box(origin: [i32; 2], columns: u32, tile: u32) -> ([f32; 2], [f32; 2]) {
    let corner = |tx: u32, ty: u32| {
        [
            (origin[0] + (tx * TILE_SIZE) as i32) as f32,
            (origin[1] + (ty * TILE_SIZE) as i32) as f32,
        ]
    };
    let [tx, ty] = [tile % columns, tile / columns];
    (corner(tx, ty), corner(tx + 1, ty + 1))
}

/// The first and last tile column and row that canvas-space `bounds` overlap, or `None` when
/// they miss the texture. Bounds that are not finite overlap every tile.
fn tile_range(bounds: [f32; 4], origin: [i32; 2], [columns, rows]: [u32; 2]) -> Option<[u32; 4]> {
    if bounds.iter().any(|value| !value.is_finite()) {
        return Some([0, 0, columns - 1, rows - 1]);
    }
    let tile = |value: f32, origin: i32| {
        ((f64::from(value) - f64::from(origin)) / f64::from(TILE_SIZE)).floor()
    };
    let [x0, y0, x1, y1] = [
        tile(bounds[0], origin[0]),
        tile(bounds[1], origin[1]),
        tile(bounds[2], origin[0]),
        tile(bounds[3], origin[1]),
    ];
    if x1 < 0.0
        || y1 < 0.0
        || x0 >= f64::from(columns)
        || y0 >= f64::from(rows)
        || x0 > x1
        || y0 > y1
    {
        return None;
    }
    let clamp = |value: f64, count: u32| value.clamp(0.0, f64::from(count - 1)) as u32;
    Some([
        clamp(x0, columns),
        clamp(y0, rows),
        clamp(x1, columns),
        clamp(y1, rows),
    ])
}

/// A grid cell lists segments, or a fill it has not decided.
const MARK_PART: u8 = 1;
/// A grid cell lies inside the fill.
const MARK_INSIDE: u8 = 2;
/// A grid cell lies outside the fill.
const MARK_OUTSIDE: u8 = 4;

/// A cell of a path's grid that marks the tiles its canvas box overlaps, and how far that box
/// lies inside the bounds of the drawable's copies: `[min_x, min_y, max_x, max_y]`, each
/// measured from the same side of the bounds.
#[derive(Clone, Copy)]
struct Cell {
    mark: u8,
    inset: [f64; 4],
}

/// The cells of `drawable`'s grid that mark tiles, measured under `copy`. One that lists no
/// segment marks nothing for a stroke, which draws nothing there. The copies of a drawable share
/// their linear part and the box their bounds are taken around, so a cell lies as far inside
/// the bounds of each, and its box under each keeps the rounding pad of that copy's bounds.
fn path_cells(drawing: &Drawing, drawable: &GpuVectorDrawable, copy: &GpuVectorCopy) -> Vec<Cell> {
    let fill = drawable.flags & STYLE_FILL_ENABLED != 0;
    let [grid_columns, grid_rows] = drawable.grid_size;
    let side = drawable.grid_scale.map(|scale| 1.0 / scale);
    let base = [
        drawable.position[0] + drawable.grid_origin[0],
        drawable.position[1] + drawable.grid_origin[1],
    ];
    let mut cells = Vec::new();
    for y in 0..grid_rows {
        for x in 0..grid_columns {
            let entry = drawable.grid_start as usize + 2 * (y * grid_columns + x) as usize;
            let count = drawing.bands[entry + 1];
            let mark = if count & CELL_COUNT != 0 || (fill && count & CELL_FILL_DECIDED == 0) {
                MARK_PART
            } else if !fill {
                continue;
            } else if count & CELL_INSIDE != 0 {
                MARK_INSIDE
            } else {
                MARK_OUTSIDE
            };
            let min = [base[0] + x as f32 * side[0], base[1] + y as f32 * side[1]];
            let max = [min[0] + side[0], min[1] + side[1]];
            let bounds = copy.transform.canvas_bounds(min, max);
            cells.push(Cell {
                mark,
                inset: [0, 1, 2, 3]
                    .map(|side| f64::from(bounds[side]) - f64::from(copy.screen_bounds[side])),
            });
        }
    }
    cells
}

/// Marks each tile of `range` with what the path's `cells` over it hold under `copy`, and lists
/// the tiles marked in `marked`. A cell marks every tile its canvas box overlaps.
fn mark_path_tiles(
    cells: &[Cell],
    copy: &GpuVectorCopy,
    origin: [i32; 2],
    tiles: [u32; 2],
    [rx0, ry0, rx1, ry1]: [u32; 4],
    marks: &mut [u8],
    marked: &mut Vec<u32>,
) {
    for cell in cells {
        let bounds = [0, 1, 2, 3]
            .map(|side| (f64::from(copy.screen_bounds[side]) + cell.inset[side]) as f32);
        let Some([x0, y0, x1, y1]) = tile_range(bounds, origin, tiles) else {
            continue;
        };
        for ty in y0.max(ry0)..=y1.min(ry1) {
            for tx in x0.max(rx0)..=x1.min(rx1) {
                let tile = ty * tiles[0] + tx;
                if marks[tile as usize] == 0 {
                    marked.push(tile);
                }
                marks[tile as usize] |= cell.mark;
            }
        }
    }
}

/// A path covers all of a tile its cells marked only inside, when the tile lies within the grid,
/// past which nothing is drawn; it covers nothing of one they marked only outside.
fn path_coverage(
    (drawable, copy): (&GpuVectorDrawable, &GpuVectorCopy),
    mark: u8,
    min: [f32; 2],
    max: [f32; 2],
) -> Coverage {
    match mark {
        MARK_OUTSIDE => Coverage::Nothing,
        MARK_INSIDE => {
            let low = [
                drawable.position[0] + drawable.grid_origin[0],
                drawable.position[1] + drawable.grid_origin[1],
            ];
            let high = [
                low[0] + drawable.grid_size[0] as f32 / drawable.grid_scale[0],
                low[1] + drawable.grid_size[1] as f32 / drawable.grid_scale[1],
            ];
            let within = |corner: [f32; 2]| {
                let local = copy.transform.to_local(corner);
                (low[0]..=high[0]).contains(&local[0]) && (low[1]..=high[1]).contains(&local[1])
            };
            if [min, [max[0], min[1]], [min[0], max[1]], max]
                .into_iter()
                .all(within)
            {
                Coverage::All
            } else {
                Coverage::Part
            }
        }
        _ => Coverage::Part,
    }
}

/// A shape's fill covers the tile where all of the tile lies within it, clear of its rounded
/// corners, any shrinking offset and a pixel of antialiasing; anything else drawn of a shape
/// covers part of it. Its bounds already left out tiles it misses.
fn shape_coverage(
    (drawable, copy): (&GpuVectorDrawable, &GpuVectorCopy),
    ellipse: bool,
    min: [f32; 2],
    max: [f32; 2],
) -> Coverage {
    if drawable.flags & STYLE_FILL_ENABLED == 0 || drawable.flags & STYLE_SHADOW != 0 {
        return Coverage::Part;
    }
    let half = drawable.size.map(|side| side.max(1.0) * 0.5);
    let centre = [
        drawable.position[0] + half[0],
        drawable.position[1] + half[1],
    ];
    let smallest = scales(&copy.transform)[0].max(1e-12) as f32;
    let radius = if ellipse {
        0.0
    } else {
        drawable.border_radius.min(half[0].min(half[1]))
    };
    let inset = radius + (-drawable.fill_offset).max(0.0) + 1.0 / smallest;
    let inner = [half[0] - inset, half[1] - inset];
    if inner[0] <= 0.0 || inner[1] <= 0.0 {
        return Coverage::Part;
    }
    let inside = |corner: [f32; 2]| {
        let local = copy.transform.to_local(corner);
        let p = [local[0] - centre[0], local[1] - centre[1]];
        if ellipse {
            (p[0] / inner[0]).powi(2) + (p[1] / inner[1]).powi(2) <= 1.0
        } else {
            p[0].abs() <= inner[0] && p[1].abs() <= inner[1]
        }
    };
    if [min, [max[0], min[1]], [min[0], max[1]], max]
        .into_iter()
        .all(inside)
    {
        Coverage::All
    } else {
        Coverage::Part
    }
}
