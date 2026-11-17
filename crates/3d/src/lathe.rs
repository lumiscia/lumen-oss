//! Solids spun around the Y axis from flat profiles.

use std::f32::consts::TAU;

use glam::{Vec2, Vec3};

use crate::{ExtrudeLimit, ExtrudeLimits, LimitExceeded, Mesh, Vertex};

/// Neighbouring edges whose directions turn by more than this many degrees meet in a hard
/// corner; others share one smooth normal.
const CORNER_ANGLE: f32 = 30.0;
/// Distance from the axis under which a point lies on it.
const AXIS_EPSILON: f32 = 1e-6;
/// Sweeps this close to a full turn or more close the solid around the axis.
const FULL_TURN: f32 = TAU - 1e-4;

/// How a profile is spun around the Y axis, which a point `(x, y)` of the profile orbits at
/// distance `x` and height `y`, starting on +X and turning towards -Z.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Lathe {
    /// Steps around the axis for the whole sweep, at least 3.
    pub segments: u32,
    /// How far around the axis the profile turns, in radians. Sweeps of a full turn or more
    /// close the solid; shorter ones leave an open wedge with no end caps.
    pub sweep: f32,
}

/// One contour of a profile. A closed contour is a ring solid whose faces point out of the area
/// it encloses, whichever way it winds. An open one is a surface whose front faces the
/// right-hand side of its direction, with X to the right and Y up.
#[derive(Debug, Clone, PartialEq)]
pub struct LatheContour {
    pub points: Vec<Vec2>,
    pub closed: bool,
}

/// What bounds the work and the output of [`lathe`], checked before it allocates beyond them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LatheLimits {
    /// Most contours in the profile.
    pub max_contours: usize,
    /// Most points over all contours.
    pub max_points: usize,
    /// Most vertices in the mesh, which is the vertices of the profile times the steps around.
    pub max_vertices: usize,
    /// Most indices in the mesh.
    pub max_indices: usize,
}

impl LatheLimits {
    /// Limits that no profile reaches.
    pub const NONE: Self = Self {
        max_contours: usize::MAX,
        max_points: usize::MAX,
        max_vertices: usize::MAX,
        max_indices: usize::MAX,
    };
}

/// A contour cleaned for spinning: no repeated points, and a closed one wound counter-clockwise
/// so that the normals of its edges, to the right of their direction, point out.
struct Cleaned {
    points: Vec<Vec2>,
    closed: bool,
}

impl Cleaned {
    fn new(contour: &LatheContour) -> Option<Self> {
        let mut points: Vec<Vec2> = contour
            .points
            .iter()
            .filter(|point| point.is_finite())
            .map(|point| Vec2::new(point.x.max(0.0), point.y))
            .map(|point| {
                if point.x <= AXIS_EPSILON {
                    Vec2::new(0.0, point.y)
                } else {
                    point
                }
            })
            .collect();
        points.dedup_by(|a, b| a.distance_squared(*b) <= 1e-12);
        if contour.closed {
            while points.len() > 1 && points[0].distance_squared(points[points.len() - 1]) <= 1e-12
            {
                points.pop();
            }
            let area = signed_area(&points);
            if points.len() < 3 || area.abs() <= 1e-12 {
                return None;
            }
            if area < 0.0 {
                points.reverse();
            }
        } else if points.len() < 2 {
            return None;
        }
        Some(Self {
            points,
            closed: contour.closed,
        })
    }

    fn edge_count(&self) -> usize {
        if self.closed {
            self.points.len()
        } else {
            self.points.len() - 1
        }
    }

    /// Edge `i`'s end points and outward normal, or `None` when it lies on the axis, where
    /// spinning leaves it no area.
    fn edge(&self, i: usize) -> Option<(Vec2, Vec2)> {
        let n = self.points.len();
        let (a, b) = (self.points[i], self.points[(i + 1) % n]);
        if a.x == 0.0 && b.x == 0.0 {
            return None;
        }
        let d = b - a;
        Some((Vec2::new(d.y, -d.x).normalize_or_zero(), d))
    }
}

fn signed_area(points: &[Vec2]) -> f32 {
    let n = points.len();
    (0..n)
        .map(|i| points[i].perp_dot(points[(i + 1) % n]))
        .sum::<f32>()
        * 0.5
}

/// The profile's vertices, which are spun into a column each, and its edges between them.
#[derive(Default)]
struct Profile {
    /// Distance from the axis, height, and the unit normal in the same two directions.
    vertices: Vec<(Vec2, Vec2)>,
    edges: Vec<[u32; 2]>,
}

impl Profile {
    fn add(&mut self, contour: &Cleaned) {
        let n = contour.points.len();
        let corner_cos = CORNER_ANGLE.to_radians().cos();
        let normals: Vec<Option<Vec2>> = (0..contour.edge_count())
            .map(|i| contour.edge(i).map(|(normal, _)| normal))
            .collect();
        // The vertex each point has for the edge ending at it, and for the one starting there.
        let mut ends: Vec<Option<u32>> = vec![None; n];
        let mut starts: Vec<Option<u32>> = vec![None; n];
        for v in 0..n {
            let before = if v > 0 || contour.closed {
                normals[(v + n - 1) % n]
            } else {
                None
            };
            let after = normals.get(v).copied().flatten();
            let point = contour.points[v];
            let mut push = |normal: Vec2| {
                self.vertices.push((point, normal));
                self.vertices.len() as u32 - 1
            };
            match (before, after) {
                (Some(before), Some(after)) if before.dot(after) > corner_cos => {
                    let shared = (before + after).normalize_or(after);
                    let slot = push(shared);
                    ends[v] = Some(slot);
                    starts[v] = Some(slot);
                }
                (before, after) => {
                    ends[v] = before.map(&mut push);
                    starts[v] = after.map(&mut push);
                }
            }
        }
        for (i, normal) in normals.iter().enumerate() {
            if normal.is_some()
                && let (Some(from), Some(to)) = (starts[i], ends[(i + 1) % n])
            {
                self.edges.push([from, to]);
            }
        }
    }
}

/// The profile `contours` spun into a solid around the Y axis: each contour becomes a strip of
/// quads around it, shaded smoothly around the axis, and along the profile except at corners
/// sharper than 30°. A point on the axis is a pole where the strip comes to a point, with no
/// triangle left without area, and an edge along the axis draws nothing. Points left of the axis
/// are put on it; points that aren't finite, repeated points and closed contours without area are
/// left out, so a degenerate profile gives an empty mesh.
///
/// Fails with the first of `limits` that the profile goes past, before allocating beyond it.
pub fn lathe(
    contours: &[LatheContour],
    lathe: &Lathe,
    limits: &LatheLimits,
) -> Result<Mesh, LimitExceeded> {
    ExtrudeLimits::check(limits.max_contours, contours.len(), ExtrudeLimit::Contours)?;
    let points = contours.iter().map(|c| c.points.len()).sum();
    ExtrudeLimits::check(limits.max_points, points, ExtrudeLimit::Points)?;
    let sweep = if lathe.sweep.is_nan() {
        0.0
    } else {
        lathe.sweep.clamp(0.0, TAU)
    };
    if sweep <= 0.0 {
        return Ok(Mesh::default());
    }
    let mut profile = Profile::default();
    for contour in contours.iter().filter_map(Cleaned::new) {
        profile.add(&contour);
    }
    if profile.edges.is_empty() {
        return Ok(Mesh::default());
    }

    let segments = lathe.segments.max(3) as usize;
    let closed = sweep >= FULL_TURN;
    let columns = if closed { segments } else { segments + 1 };
    ExtrudeLimits::check(
        limits.max_vertices,
        profile.vertices.len().saturating_mul(columns),
        ExtrudeLimit::Vertices,
    )?;
    ExtrudeLimits::check(
        limits.max_indices,
        profile
            .edges
            .len()
            .saturating_mul(segments)
            .saturating_mul(6),
        ExtrudeLimit::Indices,
    )?;

    let width = profile.vertices.len();
    let mut mesh = Mesh {
        vertices: Vec::with_capacity(width * columns),
        indices: Vec::new(),
    };
    for column in 0..columns {
        let angle = if closed && column == 0 {
            0.0
        } else {
            sweep * column as f32 / segments as f32
        };
        let (sin, cos) = angle.sin_cos();
        let around = |v: Vec2, y: f32| Vec3::new(v.x * cos, y, -v.x * sin);
        for &(point, normal) in &profile.vertices {
            mesh.vertices.push(Vertex {
                position: around(Vec2::new(point.x, 0.0), point.y).into(),
                normal: around(normal, normal.y).normalize_or_zero().into(),
                uv: [0.0; 2],
            });
        }
    }
    let at = |column: usize, slot: u32| (column * width) as u32 + slot;
    for column in 0..segments {
        let next = (column + 1) % columns;
        for &[from, to] in &profile.edges {
            let (a, b) = (at(column, from), at(column, to));
            let (d, c) = (at(next, from), at(next, to));
            let on_axis = |slot: u32| profile.vertices[slot as usize].0.x == 0.0;
            if on_axis(from) {
                mesh.indices.extend([a, c, b]);
            } else if on_axis(to) {
                mesh.indices.extend([a, d, c]);
            } else {
                mesh.indices.extend([a, d, c, a, c, b]);
            }
        }
    }
    Ok(mesh)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn spin(contours: &[LatheContour], segments: u32, sweep: f32) -> Mesh {
        lathe(contours, &Lathe { segments, sweep }, &LatheLimits::NONE).unwrap()
    }

    fn closed(points: &[(f32, f32)]) -> LatheContour {
        LatheContour {
            points: points.iter().map(|&(x, y)| Vec2::new(x, y)).collect(),
            closed: true,
        }
    }

    fn position(mesh: &Mesh, index: u32) -> Vec3 {
        mesh.vertices[index as usize].position.into()
    }

    fn triangles(mesh: &Mesh) -> impl Iterator<Item = [Vec3; 3]> + '_ {
        mesh.indices
            .chunks(3)
            .map(|t| [0, 1, 2].map(|i| position(mesh, t[i])))
    }

    fn signed_volume(mesh: &Mesh) -> f32 {
        triangles(mesh)
            .map(|[a, b, c]| a.dot(b.cross(c)) / 6.0)
            .sum()
    }

    /// Every directed edge between positions is matched by its reverse exactly once.
    fn assert_watertight(mesh: &Mesh) {
        let key = |p: Vec3| p.to_array().map(|c| (c * 1e4).round() as i64);
        let mut edges: HashMap<([i64; 3], [i64; 3]), i32> = HashMap::new();
        for [a, b, c] in triangles(mesh) {
            for (from, to) in [(a, b), (b, c), (c, a)] {
                let (from, to) = (key(from), key(to));
                if from != to {
                    *edges.entry((from, to)).or_default() += 1;
                }
            }
        }
        for (&(from, to), &count) in &edges {
            assert_eq!(count, 1, "edge {from:?} -> {to:?} drawn {count} times");
            assert_eq!(
                edges.get(&(to, from)),
                Some(&1),
                "{from:?} -> {to:?} is open"
            );
        }
    }

    #[test]
    fn a_rectangle_profile_spins_into_a_watertight_ring_facing_outwards() {
        let ring = closed(&[(1.0, 0.0), (2.0, 0.0), (2.0, 1.0), (1.0, 1.0)]);
        let mesh = spin(&[ring], 48, TAU);
        assert_watertight(&mesh);
        // The 48-sided ring holds the volume of its chords, a little under that of the circles.
        let expected = 48.0 * 0.5 * (4.0 - 1.0) * (TAU / 48.0).sin();
        assert!((signed_volume(&mesh) - expected).abs() < 1e-2);
        for triangle in mesh.indices.chunks(3) {
            let [a, b, c] = [0, 1, 2].map(|i| mesh.vertices[triangle[i] as usize]);
            let [pa, pb, pc] = [a, b, c].map(|v| Vec3::from(v.position));
            let face = (pb - pa).cross(pc - pa);
            let normal = [a, b, c]
                .map(|v| Vec3::from(v.normal))
                .into_iter()
                .sum::<Vec3>();
            assert!(face.dot(normal) > 0.0, "{triangle:?} faces inwards");
        }
        let radii: Vec<f32> = mesh
            .vertices
            .iter()
            .map(|v| v.position[0].hypot(v.position[2]))
            .collect();
        assert!(radii.iter().all(|r| (1.0 - 1e-5..=2.0 + 1e-5).contains(r)));
    }

    #[test]
    fn points_on_the_axis_are_poles_with_no_flat_triangles_and_closed_by_it() {
        // A cone: its base, its slant, and an edge back down the axis that draws nothing.
        let cone = closed(&[(0.0, 0.0), (1.0, 0.0), (0.0, 1.0)]);
        let mesh = spin(std::slice::from_ref(&cone), 32, TAU);
        assert_watertight(&mesh);
        for [a, b, c] in triangles(&mesh) {
            assert!((b - a).cross(c - a).length() > 1e-6, "flat {a} {b} {c}");
        }
        let expected = std::f32::consts::PI / 3.0;
        assert!((signed_volume(&mesh) - expected).abs() < 0.02 * expected);
        // Negative X is put on the axis, which makes the same cone.
        let pushed = closed(&[(-1.0, 0.0), (1.0, 0.0), (-3.0, 1.0), (0.0, 1.0)]);
        let flat = spin(&[pushed], 32, TAU);
        assert_eq!(flat, mesh);
    }

    #[test]
    fn a_partial_sweep_is_an_open_wedge_and_an_open_profile_is_a_surface() {
        let line = LatheContour {
            points: vec![Vec2::new(1.0, 0.0), Vec2::new(1.0, 2.0)],
            closed: false,
        };
        let half = spin(std::slice::from_ref(&line), 8, TAU / 2.0);
        assert_eq!(half.vertices.len(), 2 * 9);
        assert_eq!(half.indices.len(), 8 * 6);
        let end = position(&half, 16);
        assert!((end - Vec3::new(-1.0, 0.0, 0.0)).length() < 1e-5, "{end}");
        let full = spin(&[line], 8, TAU);
        assert_eq!(full.vertices.len(), 2 * 8);
        // Drawn upwards at x = 1, its front faces outwards.
        let normal = Vec3::from(full.vertices[0].normal);
        assert!((normal - Vec3::X).length() < 1e-5, "{normal}");
    }

    #[test]
    fn corners_sharper_than_30_degrees_are_hard_and_gentle_ones_are_smooth() {
        let gentle = closed(&[(1.0, 0.0), (2.0, 0.0), (3.0, 0.2), (3.0, 1.0), (1.0, 1.0)]);
        let hard = closed(&[(1.0, 0.0), (2.0, 0.0), (2.0, 1.0), (1.0, 1.0)]);
        let count = |contour| spin(&[contour], 3, TAU).vertices.len() / 3;
        assert_eq!(count(hard), 8);
        assert_eq!(count(gentle), 9);
    }

    #[test]
    fn a_profile_past_a_limit_fails_before_it_is_built() {
        let ring = closed(&[(1.0, 0.0), (2.0, 0.0), (2.0, 1.0), (1.0, 1.0)]);
        let lathe_of = |segments| Lathe {
            segments,
            sweep: TAU,
        };
        let limits = LatheLimits {
            max_vertices: 64,
            ..LatheLimits::NONE
        };
        assert!(lathe(std::slice::from_ref(&ring), &lathe_of(8), &limits).is_ok());
        let error = lathe(std::slice::from_ref(&ring), &lathe_of(9), &limits).unwrap_err();
        assert_eq!(error.limit, ExtrudeLimit::Vertices);
        let points = LatheLimits {
            max_points: 3,
            ..LatheLimits::NONE
        };
        let error = lathe(&[ring], &lathe_of(8), &points).unwrap_err();
        assert_eq!(error.limit, ExtrudeLimit::Points);
    }
}
