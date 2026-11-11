//! Solids extruded from flat outlines, such as a font's glyphs.

use std::collections::HashMap;
use std::f32::consts::FRAC_PI_2;
use std::fmt;

use glam::{Vec2, Vec3};
use lyon_tessellation::path::{PathEvent, math::point};
use lyon_tessellation::{
    FillGeometryBuilder, FillOptions, FillRule, FillTessellator, FillVertex, GeometryBuilder,
    GeometryBuilderError, TessellationError, VertexId,
};

use crate::{Mesh, Vertex};

/// How an outline in the XY plane is extruded along Z.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Extrusion {
    /// Thickness along Z, centred on Z = 0.
    pub depth: f32,
    /// How far the bevel reaches in from the outline and in from each cap, at most half the
    /// depth; 0 is none. The outline keeps its size where the side walls meet the bevel. An
    /// outline too thin or too tightly curved for the bevel gets the widest one that doesn't fold
    /// over itself.
    pub bevel: f32,
    /// Steps of the bevel's quarter-round profile; 1 is a flat 45° chamfer.
    pub bevel_segments: u32,
}

/// What bounds the work and the output of [`extrude`], checked while it builds so that an outline
/// is refused before it takes more than its share of time or memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExtrudeLimits {
    /// Most contours in the outline.
    pub max_contours: usize,
    /// Most points over all contours, before and after joining overlapping ones.
    pub max_points: usize,
    /// Most points over all contours that a bevel is fitted to. Fitting a bevel tests contours
    /// against each other, which takes time quadratic in their points.
    pub max_bevel_points: usize,
    /// Most vertices in the mesh. Triangulating the outline stops at half this many, which each
    /// cap takes.
    pub max_vertices: usize,
    /// Most indices in the mesh. Triangulating the outline stops at half this many.
    pub max_indices: usize,
}

/// Which of [`ExtrudeLimits`] an outline went past.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtrudeLimit {
    Contours,
    Points,
    BevelPoints,
    /// Points where contours cross each other or themselves, which joining the contours adds.
    Crossings,
    Vertices,
    Indices,
}

/// An outline that [`extrude`] refused for going past one of its [`ExtrudeLimits`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LimitExceeded {
    pub limit: ExtrudeLimit,
    /// The most the limit allows.
    pub max: usize,
}

impl fmt::Display for LimitExceeded {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let what = match self.limit {
            ExtrudeLimit::Contours => "contours",
            ExtrudeLimit::Points => "points",
            ExtrudeLimit::BevelPoints => "points to fit a bevel to",
            ExtrudeLimit::Crossings => "points where its contours cross",
            ExtrudeLimit::Vertices => "vertices",
            ExtrudeLimit::Indices => "indices",
        };
        write!(f, "more than {} {what}", self.max)
    }
}

impl std::error::Error for LimitExceeded {}

impl ExtrudeLimits {
    /// Limits that no outline reaches.
    pub const NONE: Self = Self {
        max_contours: usize::MAX,
        max_points: usize::MAX,
        max_bevel_points: usize::MAX,
        max_vertices: usize::MAX,
        max_indices: usize::MAX,
    };

    fn check(max: usize, count: usize, limit: ExtrudeLimit) -> Result<(), LimitExceeded> {
        if count > max {
            Err(LimitExceeded { limit, max })
        } else {
            Ok(())
        }
    }
}

/// Neighbouring edges whose directions turn by more than this many degrees meet in a hard
/// corner; others share one smooth normal.
const CORNER_ANGLE: f32 = 30.0;
/// Points that joining overlapping contours may add, per point of the contours, besides 512.
const CROSSING_ROOM: usize = 4;
/// The farthest a corner's bevel moves in, in bevel widths, at sharp corners.
const MAX_MITER: f32 = 3.0;
/// Halvings of the bevel tried when it is too wide for the outline.
const BEVEL_HALVINGS: u32 = 6;
/// How much wider than a bevel the outline must take without folding, so that the caps keep
/// clear of slivers.
const BEVEL_CLEARANCE: f32 = 1.25;

/// The outline's closed `contours` extruded into a solid: a front cap facing +Z, a back cap
/// facing -Z and side walls facing out around the area the contours fill by the nonzero rule, so
/// the holes of a glyph such as "o" stay open whichever way its contours wind, and contours that
/// overlap are joined. Points that aren't finite, points repeating the one before and contours
/// without area are left out, so a degenerate outline gives an empty mesh.
///
/// Fails with the first of `limits` that the outline goes past, before allocating beyond it. A
/// bevel is fitted only to outlines of up to `max_bevel_points`; a larger one fails rather than
/// going without.
pub fn extrude(
    contours: &[Vec<Vec2>],
    extrusion: &Extrusion,
    limits: &ExtrudeLimits,
) -> Result<Mesh, LimitExceeded> {
    ExtrudeLimits::check(limits.max_contours, contours.len(), ExtrudeLimit::Contours)?;
    let points = |contours: &[Vec<Vec2>]| contours.iter().map(Vec::len).sum::<usize>();
    ExtrudeLimits::check(limits.max_points, points(contours), ExtrudeLimit::Points)?;
    let contours: Vec<Vec<Vec2>> = contours.iter().filter_map(|c| clean(c)).collect();
    let contours: Vec<Vec<Vec2>> = outlines(&contours, limits)?
        .iter()
        .filter_map(|c| clean(c))
        .collect();
    let mut mesh = Mesh::default();
    if contours.is_empty() {
        return Ok(mesh);
    }
    ExtrudeLimits::check(limits.max_contours, contours.len(), ExtrudeLimit::Contours)?;
    ExtrudeLimits::check(limits.max_points, points(&contours), ExtrudeLimit::Points)?;

    let half = extrusion.depth.max(0.0) * 0.5;
    let segments = extrusion.bevel_segments.max(1);
    let bevel = if extrusion.bevel_segments == 0 {
        0.0
    } else {
        extrusion.bevel.clamp(0.0, half)
    };
    let corners: Vec<Corners> = contours.iter().map(|c| Corners::of(c)).collect();
    let bevel = if bevel > 0.0 {
        ExtrudeLimits::check(
            limits.max_bevel_points,
            points(&contours),
            ExtrudeLimit::BevelPoints,
        )?;
        fit_bevel(&contours, &corners, bevel)
    } else {
        0.0
    };
    let profile = Profile::new(half, bevel, segments);

    let mut caps = Vec::with_capacity(contours.len());
    for (contour, corners) in contours.iter().zip(&corners) {
        let rings: Vec<Vec<Vec2>> = profile
            .insets
            .iter()
            .map(|&inset| corners.inset(contour, inset))
            .collect();
        for band in &profile.bands {
            wall(&mut mesh, corners, &rings, band, limits)?;
        }
        caps.push(rings.last().cloned().unwrap_or_default());
    }
    fill_caps(&mut mesh, &caps, half, limits)?;
    Ok(mesh)
}

/// `contour` without points that aren't finite or that repeat the one before, or `None` when
/// fewer than three points or no area are left.
fn clean(contour: &[Vec2]) -> Option<Vec<Vec2>> {
    const EPSILON: f32 = 1e-12;
    let mut points: Vec<Vec2> = contour.iter().copied().filter(|p| p.is_finite()).collect();
    points.dedup_by(|a, b| a.distance_squared(*b) <= EPSILON);
    while points.len() > 1 && points[0].distance_squared(points[points.len() - 1]) <= EPSILON {
        points.pop();
    }
    (points.len() >= 3 && signed_area(&points).abs() > EPSILON).then_some(points)
}

fn signed_area(points: &[Vec2]) -> f32 {
    let n = points.len();
    (0..n)
        .map(|i| points[i].perp_dot(points[(i + 1) % n]))
        .sum::<f32>()
        * 0.5
}

/// Winding number of `contours` around `p`.
fn winding(p: Vec2, contours: &[Vec<Vec2>]) -> i32 {
    let mut total = 0;
    for contour in contours {
        for (i, &a) in contour.iter().enumerate() {
            let b = contour[(i + 1) % contour.len()];
            let side = (b - a).perp_dot(p - a);
            if a.y <= p.y {
                if b.y > p.y && side > 0.0 {
                    total += 1;
                }
            } else if b.y <= p.y && side < 0.0 {
                total -= 1;
            }
        }
    }
    total
}

/// The boundary of the area `contours` fill by the nonzero rule, as closed contours with that
/// area on their left and their outside on the right of each edge. Where contours cross, the
/// boundary turns at the crossing.
fn outlines(
    contours: &[Vec<Vec2>],
    limits: &ExtrudeLimits,
) -> Result<Vec<Vec<Vec2>>, LimitExceeded> {
    // Overlaps in a real outline add a few points each, so joining its contours is given little
    // room beyond their own points, which stops outlines that cross themselves a great many
    // times before the work grows with the square of their points.
    let input_points: usize = contours.iter().map(Vec::len).sum();
    let allowed = input_points
        .saturating_mul(CROSSING_ROOM)
        .saturating_add(512);
    let (points, triangles) = tessellate(
        contours,
        allowed.min(limits.max_vertices / 2),
        limits.max_indices / 2,
    )
    .map_err(|limit| {
        if limit == ExtrudeLimit::Vertices && allowed < limits.max_vertices / 2 {
            LimitExceeded {
                limit: ExtrudeLimit::Crossings,
                max: allowed,
            }
        } else {
            exceeded(limit, limits)
        }
    })?;
    // The edges of the triangles, wound counter-clockwise, that no neighbour shares are the
    // boundary.
    let mut edges: HashMap<(u32, u32), u32> = HashMap::default();
    for triangle in triangles.as_chunks::<3>().0 {
        for i in 0..3 {
            let (a, b) = (triangle[i], triangle[(i + 1) % 3]);
            if a != b {
                *edges.entry((a, b)).or_default() += 1;
            }
        }
    }
    let mut next: HashMap<u32, Vec<u32>> = HashMap::default();
    let mut boundary: Vec<(u32, u32)> = edges
        .keys()
        .filter(|&&(a, b)| !edges.contains_key(&(b, a)))
        .copied()
        .collect();
    boundary.sort_unstable();
    for &(a, b) in &boundary {
        next.entry(a).or_default().push(b);
    }
    let mut outlines = Vec::new();
    for (start, _) in boundary {
        let mut loop_points = Vec::new();
        let mut at = start;
        while let Some(to) = next.get_mut(&at).and_then(Vec::pop) {
            loop_points.push(points[at as usize]);
            at = to;
            if at == start {
                break;
            }
        }
        if !loop_points.is_empty() {
            outlines.push(loop_points);
        }
    }
    Ok(outlines)
}

/// A point just left of the middle of `contour`'s longest edge.
fn left_of(contour: &[Vec2]) -> Vec2 {
    let n = contour.len();
    let (a, b) = (0..n)
        .map(|i| (contour[i], contour[(i + 1) % n]))
        .max_by(|(a0, b0), (a1, b1)| {
            a0.distance_squared(*b0)
                .total_cmp(&a1.distance_squared(*b1))
        })
        .unwrap_or_default();
    (a + b) * 0.5 + (b - a).perp() * 1e-3
}

/// The outward normal of each edge of a contour from [`outlines`] and how its corners are shaded.
struct Corners {
    /// Of the edge from each point to the next.
    edges: Vec<Vec2>,
    /// Whether each point is a smooth corner, where its two edges share a normal.
    smooth: Vec<bool>,
    /// How far, per unit of inset, each point moves in.
    miters: Vec<Vec2>,
}

impl Corners {
    fn of(contour: &[Vec2]) -> Self {
        let n = contour.len();
        let edges: Vec<Vec2> = (0..n)
            .map(|i| {
                let d = contour[(i + 1) % n] - contour[i];
                Vec2::new(d.y, -d.x).normalize_or_zero()
            })
            .collect();
        let corner_cos = CORNER_ANGLE.to_radians().cos();
        let (mut smooth, mut miters) = (Vec::with_capacity(n), Vec::with_capacity(n));
        for v in 0..n {
            let (before, after) = (edges[(v + n - 1) % n], edges[v]);
            smooth.push(before.dot(after) > corner_cos);
            let mut miter = (before + after) / (1.0 + before.dot(after)).max(1e-3);
            if miter.length() > MAX_MITER {
                miter = miter.normalize_or_zero() * MAX_MITER;
            }
            miters.push(miter);
        }
        Self {
            edges,
            smooth,
            miters,
        }
    }

    /// The contour moved in by `inset`.
    fn inset(&self, contour: &[Vec2], inset: f32) -> Vec<Vec2> {
        if inset == 0.0 {
            return contour.to_vec();
        }
        contour
            .iter()
            .zip(&self.miters)
            .map(|(&p, &miter)| p - miter * inset)
            .collect()
    }

    /// The normal that the walls of point `v` take: the same for both its edges at a smooth
    /// corner, and else that of the edge ending there and that of the edge starting there.
    fn normals(&self, v: usize) -> (Vec2, Vec2) {
        let n = self.edges.len();
        let (before, after) = (self.edges[(v + n - 1) % n], self.edges[v]);
        if self.smooth[v] {
            let shared = (before + after).normalize_or(after);
            (shared, shared)
        } else {
            (before, after)
        }
    }
}

/// The widest bevel up to `bevel` that the `contours` from [`outlines`], with their `corners`, take
/// without folding, to within a [`BEVEL_HALVINGS`]th halving: no edge turns around or shrinks
/// below a tenth of its length, as one on a curve tighter than the bevel would, and the caps'
/// outline, moved in [`BEVEL_CLEARANCE`] times as far, neither crosses itself nor turns any
/// contour inside out, as the two sides of a thin stroke would where they meet or pass each
/// other.
fn fit_bevel(contours: &[Vec<Vec2>], corners: &[Corners], bevel: f32) -> f32 {
    let mut fitted = bevel;
    for (contour, corners) in contours.iter().zip(corners) {
        let n = contour.len();
        for v in 0..n {
            let next = (v + 1) % n;
            let edge = contour[next] - contour[v];
            // How much of the edge's length each unit of inset takes away.
            let shrink = (corners.miters[next] - corners.miters[v]).dot(edge);
            if shrink > 0.0 {
                fitted = fitted.min(0.9 * edge.length_squared() / shrink);
            }
        }
    }
    if fitted <= 0.0 {
        return 0.0;
    }
    let folds = |inset: f32| {
        let rings: Vec<Vec<Vec2>> = contours
            .iter()
            .zip(corners)
            .map(|(contour, corners)| corners.inset(contour, inset * BEVEL_CLEARANCE))
            .collect();
        crosses_itself(&rings) || rings.iter().any(|ring| winding(left_of(ring), &rings) == 0)
    };
    if !folds(fitted) {
        return fitted;
    }
    let (mut fits, mut too_wide) = (0.0, fitted);
    for _ in 0..BEVEL_HALVINGS {
        let middle = (fits + too_wide) * 0.5;
        if folds(middle) {
            too_wide = middle;
        } else {
            fits = middle;
        }
    }
    fits
}

/// Whether any two edges of `contours` cross, other than neighbours meeting at their point.
fn crosses_itself(contours: &[Vec<Vec2>]) -> bool {
    let mut edges: Vec<(usize, usize, Vec2, Vec2)> = contours
        .iter()
        .enumerate()
        .flat_map(|(c, contour)| {
            let n = contour.len();
            (0..n).map(move |i| (c, i, contour[i], contour[(i + 1) % n]))
        })
        .collect();
    // Sorted by their left ends, so each edge is only tested against those that overlap it in X.
    edges.sort_unstable_by(|e, f| e.2.x.min(e.3.x).total_cmp(&f.2.x.min(f.3.x)));
    let side = |a: Vec2, b: Vec2, p: Vec2| (b - a).perp_dot(p - a);
    for (index, &(c, i, a, b)) in edges.iter().enumerate() {
        let (min, max) = (a.min(b), a.max(b));
        for &(d, j, p, q) in &edges[index + 1..] {
            if p.x.min(q.x) > max.x {
                break;
            }
            if c == d {
                let n = contours[c].len();
                if j == (i + 1) % n || i == (j + 1) % n {
                    continue;
                }
            }
            if p.y.max(q.y) < min.y || p.y.min(q.y) > max.y {
                continue;
            }
            let (s1, s2) = (side(a, b, p), side(a, b, q));
            let (s3, s4) = (side(p, q, a), side(p, q, b));
            if s1 * s2 <= 0.0 && s3 * s4 <= 0.0 && (s1, s2, s3, s4) != (0.0, 0.0, 0.0, 0.0) {
                return true;
            }
        }
    }
    false
}

/// One ring of the side profile: how far in it is from the outline, where along Z it is, and its
/// normal as how much of the outward normal and of +Z it takes.
#[derive(Debug, Clone, Copy)]
struct Row {
    ring: usize,
    z: f32,
    normal: Vec2,
}

/// A strip of side wall between two rows, from the back towards the front.
#[derive(Debug, Clone, Copy)]
struct Band {
    from: Row,
    to: Row,
}

/// The rings an extrusion's side walls pass through, by how far in each is, the last at the caps,
/// and the strips between them.
struct Profile {
    insets: Vec<f32>,
    bands: Vec<Band>,
}

impl Profile {
    fn new(half: f32, bevel: f32, segments: u32) -> Self {
        let flat = half - bevel;
        if bevel <= 0.0 {
            let row = |z| Row {
                ring: 0,
                z,
                normal: Vec2::X,
            };
            return Self {
                insets: vec![0.0],
                bands: (half > 0.0)
                    .then(|| Band {
                        from: row(-half),
                        to: row(half),
                    })
                    .into_iter()
                    .collect(),
            };
        }
        let angles: Vec<f32> = (0..=segments)
            .map(|step| step as f32 / segments as f32 * FRAC_PI_2)
            .collect();
        // The last ring, at the caps, is exactly `bevel` in and `half` out.
        let last = segments as usize;
        let insets = (0..=last)
            .map(|ring| {
                if ring == last {
                    bevel
                } else {
                    bevel * (1.0 - angles[ring].cos())
                }
            })
            .collect();
        let mut bands = Vec::new();
        if flat > 0.0 {
            let row = |z| Row {
                ring: 0,
                z,
                normal: Vec2::X,
            };
            bands.push(Band {
                from: row(-flat),
                to: row(flat),
            });
        }
        for side in [1.0_f32, -1.0] {
            for step in 0..segments as usize {
                let row = |ring: usize| {
                    // A chamfer is flat; a rounder bevel is shaded smooth along its profile.
                    let angle = if segments == 1 {
                        FRAC_PI_2 * 0.5
                    } else {
                        angles[ring]
                    };
                    Row {
                        ring,
                        z: side
                            * if ring == last {
                                half
                            } else {
                                flat + bevel * angles[ring].sin()
                            },
                        normal: Vec2::new(angle.cos(), side * angle.sin()),
                    }
                };
                // Bands run from the back towards the front, so the back's run outward.
                let (inner, outer) = (row(step + 1), row(step));
                bands.push(if side > 0.0 {
                    Band {
                        from: outer,
                        to: inner,
                    }
                } else {
                    Band {
                        from: inner,
                        to: outer,
                    }
                });
            }
        }
        Self { insets, bands }
    }
}

/// Adds the band of side wall between two of `rings` around a contour, wound to face out.
fn wall(
    mesh: &mut Mesh,
    corners: &Corners,
    rings: &[Vec<Vec2>],
    band: &Band,
    limits: &ExtrudeLimits,
) -> Result<(), LimitExceeded> {
    let n = corners.edges.len();
    let hard = corners.smooth.iter().filter(|&&smooth| !smooth).count();
    ExtrudeLimits::check(
        limits.max_vertices,
        mesh.vertices.len().saturating_add(2 * (n + hard)),
        ExtrudeLimit::Vertices,
    )?;
    ExtrudeLimits::check(
        limits.max_indices,
        mesh.indices.len().saturating_add(6 * n),
        ExtrudeLimit::Indices,
    )?;
    let (from, to) = (&rings[band.from.ring], &rings[band.to.ring]);
    let vertex = |mesh: &mut Mesh, p: Vec2, z: f32, outward: Vec2, row: &Row| {
        let normal =
            (outward.extend(0.0) * row.normal.x + Vec3::Z * row.normal.y).normalize_or_zero();
        mesh.vertices.push(Vertex {
            position: [p.x, p.y, z],
            normal: normal.into(),
            uv: [0.0; 2],
        });
        mesh.vertices.len() as u32 - 1
    };
    // Each point's vertices at the band's two rows: for the edge ending there and for the edge
    // starting there, which are the same at a smooth corner.
    let mut ends = Vec::with_capacity(n);
    let mut starts = Vec::with_capacity(n);
    for v in 0..n {
        let (before, after) = corners.normals(v);
        let pair = |mesh: &mut Mesh, outward| {
            [
                vertex(mesh, from[v], band.from.z, outward, &band.from),
                vertex(mesh, to[v], band.to.z, outward, &band.to),
            ]
        };
        let end = pair(mesh, before);
        let start = if corners.smooth[v] {
            end
        } else {
            pair(mesh, after)
        };
        ends.push(end);
        starts.push(start);
    }
    for v in 0..n {
        let ([a, d], [b, c]) = (starts[v], ends[(v + 1) % n]);
        mesh.indices.extend([a, b, c, a, c, d]);
    }
    Ok(())
}

/// Adds the front cap at `half` and the back cap at `-half` over the area `contours` fill.
fn fill_caps(
    mesh: &mut Mesh,
    contours: &[Vec<Vec2>],
    half: f32,
    limits: &ExtrudeLimits,
) -> Result<(), LimitExceeded> {
    let (points, triangles) = tessellate(contours, limits.max_vertices / 2, limits.max_indices / 2)
        .map_err(|limit| exceeded(limit, limits))?;
    ExtrudeLimits::check(
        limits.max_vertices,
        mesh.vertices.len().saturating_add(2 * points.len()),
        ExtrudeLimit::Vertices,
    )?;
    ExtrudeLimits::check(
        limits.max_indices,
        mesh.indices.len().saturating_add(2 * triangles.len()),
        ExtrudeLimit::Indices,
    )?;
    for side in [1.0_f32, -1.0] {
        let base = mesh.vertices.len() as u32;
        mesh.vertices.extend(points.iter().map(|p| Vertex {
            position: [p.x, p.y, side * half],
            normal: [0.0, 0.0, side],
            uv: [0.0; 2],
        }));
        for triangle in triangles.as_chunks::<3>().0 {
            let [i, j, k] = [triangle[0], triangle[1], triangle[2]].map(|index| base + index);
            mesh.indices
                .extend(if side > 0.0 { [i, j, k] } else { [i, k, j] });
        }
    }
    Ok(())
}

/// The failure of `limit`, `Vertices` or `Indices`, of a tessellation within `limits`.
fn exceeded(limit: ExtrudeLimit, limits: &ExtrudeLimits) -> LimitExceeded {
    let max = match limit {
        ExtrudeLimit::Indices => limits.max_indices,
        _ => limits.max_vertices,
    };
    LimitExceeded { limit, max }
}

/// Collects a tessellation's points and triangles, refusing the vertex past `max_vertices`.
struct Capped {
    points: Vec<Vec2>,
    triangles: Vec<u32>,
    scale: f32,
    max_vertices: usize,
    max_indices: usize,
    too_many_indices: bool,
}

impl GeometryBuilder for Capped {
    fn add_triangle(&mut self, a: VertexId, b: VertexId, c: VertexId) {
        if self.triangles.len() + 3 > self.max_indices {
            self.too_many_indices = true;
        } else {
            self.triangles.extend([a.0, b.0, c.0]);
        }
    }
}

impl FillGeometryBuilder for Capped {
    fn add_fill_vertex(&mut self, vertex: FillVertex) -> Result<VertexId, GeometryBuilderError> {
        if self.points.len() >= self.max_vertices {
            return Err(GeometryBuilderError::TooManyVertices);
        }
        let p = vertex.position();
        self.points.push(Vec2::new(p.x, p.y) / self.scale);
        Ok(VertexId(self.points.len() as u32 - 1))
    }
}

/// The triangles covering the area `contours` fill by the nonzero rule, as points and indices
/// wound counter-clockwise, or none when the tessellator fails. Fails with the limit passed,
/// `Vertices` or `Indices`, as soon as the points go past `max_vertices` or the indices past
/// `max_indices`.
fn tessellate(
    contours: &[Vec<Vec2>],
    max_vertices: usize,
    max_indices: usize,
) -> Result<(Vec<Vec2>, Vec<u32>), ExtrudeLimit> {
    // The tessellator's own thresholds suit outlines some hundreds of units across, so outlines
    // are filled at that size, scaled by a power of two that leaves every point as exact.
    let extent = contours
        .iter()
        .flatten()
        .fold(0.0_f32, |extent, p| extent.max(p.abs().max_element()));
    let scale = if extent > 0.0 {
        2.0_f32.powi(10 - extent.log2().ceil() as i32)
    } else {
        1.0
    };
    let events = contours
        .iter()
        .filter(|c| c.len() >= 3)
        .flat_map(|contour| {
            let at = |p: Vec2| point(p.x * scale, p.y * scale);
            let first = at(contour[0]);
            std::iter::once(PathEvent::Begin { at: first })
                .chain(contour.windows(2).map(move |pair| PathEvent::Line {
                    from: at(pair[0]),
                    to: at(pair[1]),
                }))
                .chain(std::iter::once(PathEvent::End {
                    last: at(contour[contour.len() - 1]),
                    first,
                    close: true,
                }))
        });
    let mut buffers = Capped {
        points: Vec::new(),
        triangles: Vec::new(),
        scale,
        max_vertices,
        max_indices,
        too_many_indices: false,
    };
    let filled = FillTessellator::new().tessellate(
        events,
        &FillOptions::default().with_fill_rule(FillRule::NonZero),
        &mut buffers,
    );
    match filled {
        Err(TessellationError::GeometryBuilder(_)) => return Err(ExtrudeLimit::Vertices),
        Err(_) => return Ok((Vec::new(), Vec::new())),
        Ok(_) if buffers.too_many_indices => return Err(ExtrudeLimit::Indices),
        Ok(_) => {}
    }
    let (points, mut triangles) = (buffers.points, buffers.triangles);
    // The tessellator winds every triangle one way, so the total area tells which; testing each
    // triangle alone would flip the zero-area ones it leaves between points on one line.
    let area: f32 = triangles
        .as_chunks::<3>()
        .0
        .iter()
        .map(|t| {
            let [a, b, c] = [0, 1, 2].map(|i| points[t[i] as usize]);
            (b - a).perp_dot(c - a)
        })
        .sum();
    if area < 0.0 {
        for triangle in triangles.as_chunks_mut::<3>().0 {
            triangle.swap(1, 2);
        }
    }
    Ok((points, triangles))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn extrude(contours: &[Vec<Vec2>], extrusion: &Extrusion) -> Mesh {
        super::extrude(contours, extrusion, &ExtrudeLimits::NONE).unwrap()
    }

    fn rectangle(min: Vec2, max: Vec2) -> Vec<Vec2> {
        vec![min, Vec2::new(max.x, min.y), max, Vec2::new(min.x, max.y)]
    }

    fn circle(center: Vec2, radius: f32, points: usize) -> Vec<Vec2> {
        (0..points)
            .map(|i| {
                let angle = i as f32 / points as f32 * std::f32::consts::TAU;
                center + Vec2::from_angle(angle) * radius
            })
            .collect()
    }

    /// A ring like an "o": an outer circle wound counter-clockwise and its counter clockwise.
    fn ring() -> Vec<Vec<Vec2>> {
        let mut counter = circle(Vec2::ZERO, 0.5, 32);
        counter.reverse();
        vec![circle(Vec2::ZERO, 1.0, 48), counter]
    }

    fn plain(depth: f32) -> Extrusion {
        Extrusion {
            depth,
            bevel: 0.0,
            bevel_segments: 0,
        }
    }

    fn position(mesh: &Mesh, index: u32) -> Vec3 {
        mesh.vertices[index as usize].position.into()
    }

    fn signed_volume(mesh: &Mesh) -> f32 {
        mesh.indices
            .as_chunks::<3>()
            .0
            .iter()
            .map(|t| {
                let [a, b, c] = [0, 1, 2].map(|i| position(mesh, t[i]));
                a.dot(b.cross(c)) / 6.0
            })
            .sum()
    }

    /// Every directed edge between positions is matched by its reverse exactly once, so the
    /// surface is closed and consistently wound.
    fn assert_watertight(mesh: &Mesh) {
        let key = |i: u32| {
            position(mesh, i)
                .to_array()
                .map(|c| (c * 1e4).round() as i64)
        };
        let mut edges: HashMap<([i64; 3], [i64; 3]), i32> = HashMap::new();
        for t in mesh.indices.as_chunks::<3>().0 {
            for i in 0..3 {
                let (a, b) = (key(t[i]), key(t[(i + 1) % 3]));
                if a != b {
                    *edges.entry((a, b)).or_default() += 1;
                }
            }
        }
        for (&(a, b), &count) in &edges {
            assert_eq!(
                count, 1,
                "edge {a:?} → {b:?} is used twice in one direction"
            );
            assert_eq!(
                edges.get(&(b, a)),
                Some(&1),
                "edge {a:?} → {b:?} has no opposite"
            );
        }
    }

    /// Every triangle's winding agrees with its vertices' normals, so it faces out.
    fn assert_faces_out(mesh: &Mesh) {
        for t in mesh.indices.as_chunks::<3>().0 {
            let [a, b, c] = [0, 1, 2].map(|i| position(mesh, t[i]));
            let face = (b - a).cross(c - a);
            if face.length_squared() < 1e-12 {
                continue;
            }
            let normal: Vec3 = [0, 1, 2]
                .map(|i| Vec3::from(mesh.vertices[t[i] as usize].normal))
                .into_iter()
                .sum();
            assert!(face.dot(normal) > 0.0, "triangle {t:?} faces in");
        }
        for vertex in &mesh.vertices {
            assert!((Vec3::from(vertex.normal).length() - 1.0).abs() < 1e-5);
        }
    }

    #[test]
    fn a_plain_extrusion_is_a_closed_solid_facing_out() {
        let mesh = extrude(&[rectangle(Vec2::ZERO, Vec2::new(2.0, 1.0))], &plain(0.5));
        assert_watertight(&mesh);
        assert_faces_out(&mesh);
        assert!((signed_volume(&mesh) - 1.0).abs() < 1e-4);
        let z: Vec<f32> = mesh.vertices.iter().map(|v| v.position[2]).collect();
        assert!(z.iter().all(|z| z.abs() == 0.25), "centred on Z = 0");
    }

    #[test]
    fn a_counter_stays_open_whichever_way_the_outline_winds() {
        let full = std::f32::consts::PI * (1.0 - 0.25);
        // TrueType outlines wind outer contours clockwise, and CFF ones counter-clockwise.
        for reversed in [false, true] {
            let mut contours = ring();
            if reversed {
                contours.iter_mut().for_each(|contour| contour.reverse());
            }
            let mesh = extrude(&contours, &plain(1.0));
            assert_watertight(&mesh);
            assert_faces_out(&mesh);
            // The polygons' area, a little less than the circles'.
            let volume = signed_volume(&mesh);
            assert!(volume > full * 0.98 && volume < full, "volume {volume}");
            // No cap covers the middle of the counter.
            let covers_middle = mesh.indices.as_chunks::<3>().0.iter().any(|t| {
                let [a, b, c] = [0, 1, 2].map(|i| position(&mesh, t[i]).truncate());
                let sides = [(a, b), (b, c), (c, a)].map(|(p, q)| (q - p).perp_dot(-p) >= 0.0);
                sides.iter().all(|&s| s) || sides.iter().all(|&s| !s)
            });
            assert!(!covers_middle);
        }
    }

    #[test]
    fn a_bevel_keeps_the_solid_closed_and_rounds_its_edges() {
        for segments in [1, 4] {
            let extrusion = Extrusion {
                depth: 0.4,
                bevel: 0.05,
                bevel_segments: segments,
            };
            let mesh = extrude(&ring(), &extrusion);
            assert_watertight(&mesh);
            assert_faces_out(&mesh);
            // The caps are inset, but the walls still reach the outline.
            let reach = mesh
                .vertices
                .iter()
                .map(|v| Vec2::new(v.position[0], v.position[1]).length())
                .fold(0.0, f32::max);
            assert!((reach - 1.0).abs() < 1e-5, "reach {reach}");
            let front = mesh.vertices.iter().filter(|v| v.normal == [0.0, 0.0, 1.0]);
            assert!(front.into_iter().all(|v| {
                let r = Vec2::new(v.position[0], v.position[1]).length();
                r <= 1.0 - 0.05 + 1e-3
            }));
        }
    }

    #[test]
    fn a_bevel_too_wide_for_the_outline_narrows_until_it_doesnt_fold() {
        // The ring is half a unit thick, so bevels from each side meet a quarter unit in.
        let extrusion = Extrusion {
            depth: 2.0,
            bevel: 0.6,
            bevel_segments: 3,
        };
        let mesh = extrude(&ring(), &extrusion);
        assert_watertight(&mesh);
        assert_faces_out(&mesh);
        let front = mesh.vertices.iter().filter(|v| v.normal == [0.0, 0.0, 1.0]);
        let reach = front.map(|v| Vec2::new(v.position[0], v.position[1]).length());
        let bevel = 1.0 - reach.fold(f32::NAN, f32::max);
        assert!(bevel > 0.1 && bevel < 0.25, "bevel {bevel}");
    }

    #[test]
    fn smooth_curves_share_normals_and_corners_keep_their_own() {
        let mesh = extrude(&[circle(Vec2::ZERO, 1.0, 64)], &plain(1.0));
        let wall = |vertex: &Vertex| vertex.normal[2] == 0.0;
        for vertex in mesh.vertices.iter().filter(|v| wall(v)) {
            let radial = Vec2::new(vertex.position[0], vertex.position[1]).normalize();
            let normal = Vec2::new(vertex.normal[0], vertex.normal[1]);
            assert!(
                radial.dot(normal) > 0.999,
                "a circle's walls point out radially"
            );
        }
        let square = extrude(&[rectangle(Vec2::ZERO, Vec2::ONE)], &plain(1.0));
        for vertex in square.vertices.iter().filter(|v| wall(v)) {
            let [x, y, _] = vertex.normal;
            assert!(
                x == 0.0 || y == 0.0,
                "a square's walls are flat, got {x} {y}"
            );
        }
    }

    #[test]
    fn degenerate_outlines_give_an_empty_mesh() {
        let line = vec![Vec2::ZERO, Vec2::X, Vec2::new(2.0, 0.0)];
        let repeated = vec![Vec2::ZERO; 5];
        let nan = vec![Vec2::NAN, Vec2::X, Vec2::Y];
        for contours in [vec![], vec![line], vec![repeated], vec![nan]] {
            assert_eq!(extrude(&contours, &plain(1.0)), Mesh::default());
        }
        // No depth still draws both caps.
        let flat = extrude(&[rectangle(Vec2::ZERO, Vec2::ONE)], &plain(0.0));
        assert_eq!(flat.indices.len(), 12);
    }

    fn limits() -> ExtrudeLimits {
        ExtrudeLimits {
            max_contours: 64,
            max_points: 8192,
            max_bevel_points: 3072,
            max_vertices: 262_144,
            max_indices: 786_432,
        }
    }

    fn failure(
        contours: &[Vec<Vec2>],
        extrusion: &Extrusion,
        limits: &ExtrudeLimits,
    ) -> LimitExceeded {
        super::extrude(contours, extrusion, limits).expect_err("the outline is past a limit")
    }

    fn bevelled(segments: u32) -> Extrusion {
        Extrusion {
            depth: 0.4,
            bevel: 0.02,
            bevel_segments: segments,
        }
    }

    #[test]
    fn outlines_past_the_input_limits_fail_before_any_work() {
        // Fitting a bevel to thousands of small contours takes seconds.
        let triangles: Vec<Vec<Vec2>> = (0..2000)
            .map(|i| {
                let at = Vec2::new((i % 50) as f32, (i / 50) as f32) * 3.0;
                vec![at, at + Vec2::X, at + Vec2::Y]
            })
            .collect();
        let start = std::time::Instant::now();
        let error = failure(&triangles, &bevelled(4), &limits());
        assert_eq!(
            error,
            LimitExceeded {
                limit: ExtrudeLimit::Contours,
                max: 64
            }
        );
        assert!(start.elapsed().as_millis() < 50);
        assert_eq!(error.to_string(), "more than 64 contours");

        let points = failure(&[circle(Vec2::ZERO, 1.0, 9000)], &plain(1.0), &limits());
        assert_eq!(points.limit, ExtrudeLimit::Points);
    }

    #[test]
    fn a_bevel_is_fitted_only_to_outlines_within_the_bevel_limit() {
        let limits = ExtrudeLimits {
            max_bevel_points: 100,
            ..limits()
        };
        let outline = [circle(Vec2::ZERO, 1.0, 200)];
        let error = failure(&outline, &bevelled(2), &limits);
        assert_eq!(
            error,
            LimitExceeded {
                limit: ExtrudeLimit::BevelPoints,
                max: 100
            }
        );
        assert!(super::extrude(&outline, &plain(1.0), &limits).is_ok());
    }

    #[test]
    fn the_mesh_stops_at_its_vertex_and_index_limits_before_it_grows_past_them() {
        let outline = [circle(Vec2::ZERO, 1.0, 200)];
        let tight = ExtrudeLimits {
            max_vertices: 500,
            ..limits()
        };
        assert_eq!(
            failure(&outline, &bevelled(8), &tight).limit,
            ExtrudeLimit::Vertices
        );
        let tight = ExtrudeLimits {
            max_indices: 1500,
            ..limits()
        };
        assert_eq!(
            failure(&outline, &bevelled(8), &tight).limit,
            ExtrudeLimit::Indices
        );
        // A mesh that exactly fits is built.
        let mesh = extrude(&outline, &bevelled(8));
        let exact = ExtrudeLimits {
            max_vertices: mesh.vertices.len(),
            max_indices: mesh.indices.len(),
            ..limits()
        };
        assert_eq!(
            super::extrude(&outline, &bevelled(8), &exact).unwrap(),
            mesh
        );
    }

    #[test]
    fn contours_that_cross_a_great_many_times_stop_at_the_triangulation_limit() {
        // A star polygon of 1999 points each jumping 800 ahead crosses itself some 800000 times.
        let points = 1999;
        let star: Vec<Vec2> = (0..points)
            .map(|i| {
                let angle = (i * 800 % points) as f32 / points as f32 * std::f32::consts::TAU;
                Vec2::from_angle(angle)
            })
            .collect();
        let start = std::time::Instant::now();
        let error = failure(&[star], &plain(1.0), &limits());
        assert_eq!(error.limit, ExtrudeLimit::Crossings);
        assert!(start.elapsed().as_secs() < 2, "{:?}", start.elapsed());
    }
}
