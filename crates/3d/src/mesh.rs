use std::f32::consts::{PI, TAU};

use bytemuck::{Pod, Zeroable};
use glam::Vec3;

/// One vertex of a mesh, laid out as the engine's scene shader reads it.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    /// Unit length.
    pub normal: [f32; 3],
    /// Texture coordinates, with V running down like a raster's rows.
    pub uv: [f32; 2],
}

/// How a skinned vertex follows its skin's joints, kept apart from [`Vertex`] so that meshes
/// without a skin don't carry it.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Pod, Zeroable)]
pub struct SkinVertex {
    /// Indices into the skin's joints.
    pub joints: [u16; 4],
    /// How much each joint moves the vertex, as 16-bit fractions of 1.
    pub weights: [u16; 4],
}

/// What a model's vertices hold beyond [`Vertex`], kept apart so that meshes without it don't carry
/// it: their second set of texture coordinates and their colour.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct VertexExtra {
    /// `TEXCOORD_1`, or the vertex's first set when its primitive has no second.
    pub uv1: [f32; 2],
    /// `COLOR_0` in linear light with straight alpha, as 16-bit fractions of 1; white without one.
    pub color: [u16; 4],
}

/// How far one morph target moves one vertex at a weight of 1.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Pod, Zeroable)]
pub struct MorphDelta {
    pub position: [f32; 3],
    pub normal: [f32; 3],
}

/// Triangles over shared vertices, wound counter-clockwise seen from the side their normals face.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Mesh {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Primitive {
    Box,
    Sphere,
    /// A square in the XZ plane facing +Y.
    Plane,
    Cylinder,
    Torus,
    Cone,
}

/// A face of the box [`Mesh::primitive`] builds, as its axes stand before any rotation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoxFace {
    /// +X.
    Right,
    /// -X.
    Left,
    /// +Y.
    Top,
    /// -Y.
    Bottom,
    /// +Z, the face toward a camera looking down -Z.
    Front,
    /// -Z.
    Back,
}

impl BoxFace {
    /// The faces in the order their indices come in the box's mesh, six to a face.
    pub const ALL: [Self; 6] = [
        Self::Right,
        Self::Left,
        Self::Top,
        Self::Bottom,
        Self::Front,
        Self::Back,
    ];

    /// The range of the box's indices that draw this face.
    pub fn indices(self) -> std::ops::Range<u32> {
        let first = Self::ALL.iter().position(|&face| face == self).unwrap_or(0) as u32 * 6;
        first..first + 6
    }
}

const SEGMENTS: u32 = 64;
const RINGS: u32 = 32;

/// The scale that turns [`Mesh::textured_quad`] into a plane `width` across with the aspect ratio of a
/// raster `display` pixels in size. A raster with no area gets a plane with none.
pub fn plane_scale(width: f32, display: (u32, u32)) -> Vec3 {
    let (across, down) = (display.0 as f32, display.1 as f32);
    let height = if across > 0.0 {
        width * down / across
    } else {
        0.0
    };
    Vec3::new(width, height, 1.0)
}

impl Mesh {
    /// A unit square in the XY plane facing +Z, textured edge to edge.
    pub fn textured_quad() -> Self {
        let mut mesh = Self::default();
        for (x, y) in [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)] {
            mesh.vertex_uv(Vec3::new(x - 0.5, y - 0.5, 0.0), Vec3::Z, [x, 1.0 - y]);
        }
        mesh.quad(0, 1, 2, 3);
        mesh
    }

    /// `primitive` one unit across in X and Z, centred on the origin. Round primitives stand along
    /// Y, a unit tall but for the flat plane and the torus, whose tube is a quarter unit thick.
    /// Their texture coordinates are laid out as the engine's `shape_3d` documents: round ones run
    /// U around Y from -Z, towards -X.
    pub fn primitive(primitive: Primitive) -> Self {
        let mut mesh = Self::default();
        match primitive {
            Primitive::Box => {
                let faces = [
                    (Vec3::X, Vec3::NEG_Z, Vec3::Y),
                    (Vec3::NEG_X, Vec3::Z, Vec3::Y),
                    (Vec3::Y, Vec3::X, Vec3::NEG_Z),
                    (Vec3::NEG_Y, Vec3::X, Vec3::Z),
                    (Vec3::Z, Vec3::X, Vec3::Y),
                    (Vec3::NEG_Z, Vec3::NEG_X, Vec3::Y),
                ];
                for (normal, right, up) in faces {
                    let base = mesh.vertices.len() as u32;
                    for (x, y) in [(-0.5, -0.5), (0.5, -0.5), (0.5, 0.5), (-0.5, 0.5)] {
                        let uv = [x + 0.5, 0.5 - y];
                        mesh.vertex_uv(normal * 0.5 + right * x + up * y, normal, uv);
                    }
                    mesh.quad(base, base + 1, base + 2, base + 3);
                }
            }
            Primitive::Plane => {
                for (x, z) in [(-0.5, 0.5), (0.5, 0.5), (0.5, -0.5), (-0.5, -0.5)] {
                    mesh.vertex_uv(Vec3::new(x, 0.0, z), Vec3::Y, [x + 0.5, z + 0.5]);
                }
                mesh.quad(0, 1, 2, 3);
            }
            Primitive::Sphere => {
                for ring in 0..=RINGS {
                    let v = ring as f32 / RINGS as f32;
                    let polar = v * PI;
                    for segment in 0..=SEGMENTS {
                        let u = segment as f32 / SEGMENTS as f32;
                        let azimuth = u * TAU;
                        let normal = Vec3::new(
                            -polar.sin() * azimuth.sin(),
                            polar.cos(),
                            -polar.sin() * azimuth.cos(),
                        );
                        mesh.vertex_uv(normal * 0.5, normal, [u, v]);
                    }
                }
                mesh.grid(RINGS, SEGMENTS);
            }
            Primitive::Cylinder | Primitive::Cone => {
                let top = if primitive == Primitive::Cone {
                    0.0
                } else {
                    0.5
                };
                let slope = 0.5 - top;
                // From -Z, towards -X.
                let around = |segment: u32| {
                    let (sin, cos) = (segment as f32 / SEGMENTS as f32 * TAU).sin_cos();
                    (-sin, -cos)
                };
                for (y, radius) in [(0.5, top), (-0.5, 0.5)] {
                    for segment in 0..=SEGMENTS {
                        let (x, z) = around(segment);
                        mesh.vertex_uv(
                            Vec3::new(x * radius, y, z * radius),
                            Vec3::new(x, slope, z).normalize(),
                            [segment as f32 / SEGMENTS as f32, 0.5 - y],
                        );
                    }
                }
                mesh.grid(1, SEGMENTS);
                for (y, radius) in [(0.5, top), (-0.5, 0.5)] {
                    if radius == 0.0 {
                        continue;
                    }
                    let normal = Vec3::new(0.0, y * 2.0, 0.0);
                    // Seen from outside, top edge towards -Z on the top cap and +Z on the bottom.
                    let cap_uv = |x: f32, z: f32| [x + 0.5, 0.5 + z * normal.y];
                    let center = mesh.vertices.len() as u32;
                    mesh.vertex_uv(Vec3::new(0.0, y, 0.0), normal, cap_uv(0.0, 0.0));
                    for segment in 0..=SEGMENTS {
                        let (x, z) = around(segment);
                        let (x, z) = (x * radius, z * radius);
                        mesh.vertex_uv(Vec3::new(x, y, z), normal, cap_uv(x, z));
                    }
                    for segment in 0..SEGMENTS {
                        let (a, b) = (center + 1 + segment, center + 2 + segment);
                        if y > 0.0 {
                            mesh.indices.extend([center, a, b]);
                        } else {
                            mesh.indices.extend([center, b, a]);
                        }
                    }
                }
            }
            Primitive::Torus => {
                let (major, minor) = (0.375, 0.125);
                for ring in 0..=RINGS {
                    let v = ring as f32 / RINGS as f32;
                    // The tube angle runs from the inner equator up over the top, then down the
                    // outer equator, so the grid winds outward.
                    let (tube_sin, tube_cos) = (PI - v * TAU).sin_cos();
                    for segment in 0..=SEGMENTS {
                        let u = segment as f32 / SEGMENTS as f32;
                        let (sin, cos) = (u * TAU).sin_cos();
                        let normal = Vec3::new(-sin * tube_cos, tube_sin, -cos * tube_cos);
                        let center = Vec3::new(-sin * major, 0.0, -cos * major);
                        mesh.vertex_uv(center + normal * minor, normal, [u, v]);
                    }
                }
                mesh.grid(RINGS, SEGMENTS);
            }
        }
        mesh
    }

    fn vertex_uv(&mut self, position: Vec3, normal: Vec3, uv: [f32; 2]) {
        self.vertices.push(Vertex {
            position: position.into(),
            normal: normal.into(),
            uv,
        });
    }

    /// Two triangles of a quad wound `a`, `b`, `c`, `d`.
    fn quad(&mut self, a: u32, b: u32, c: u32, d: u32) {
        self.indices.extend([a, b, c, a, c, d]);
    }

    /// The faces of `rows` by `columns` cells over the last `rows + 1` rows of `columns + 1`
    /// vertices, from the top.
    fn grid(&mut self, rows: u32, columns: u32) {
        let start = self.vertices.len() as u32 - (rows + 1) * (columns + 1);
        for row in 0..rows {
            for column in 0..columns {
                let a = start + row * (columns + 1) + column;
                let below = a + columns + 1;
                self.quad(a, below, below + 1, a + 1);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Primitive; 6] = [
        Primitive::Box,
        Primitive::Sphere,
        Primitive::Plane,
        Primitive::Cylinder,
        Primitive::Torus,
        Primitive::Cone,
    ];

    #[test]
    fn primitives_span_a_unit_about_the_origin() {
        for primitive in ALL {
            let mesh = Mesh::primitive(primitive);
            let (mut min, mut max) = (Vec3::MAX, Vec3::MIN);
            for vertex in &mesh.vertices {
                min = min.min(vertex.position.into());
                max = max.max(vertex.position.into());
                assert!((Vec3::from(vertex.normal).length() - 1.0).abs() < 1e-5);
            }
            let half_height = match primitive {
                Primitive::Plane => 0.0,
                Primitive::Torus => 0.125,
                _ => 0.5,
            };
            let half = Vec3::new(0.5, half_height, 0.5);
            assert!(
                (max - half).abs().max_element() < 1e-5,
                "{primitive:?} max {max}"
            );
            assert!(
                (min + half).abs().max_element() < 1e-5,
                "{primitive:?} min {min}"
            );
        }
    }

    #[test]
    fn triangles_wind_counter_clockwise_towards_their_normals() {
        for primitive in ALL {
            let mesh = Mesh::primitive(primitive);
            assert_eq!(mesh.indices.len() % 3, 0);
            for triangle in mesh.indices.chunks(3) {
                let [a, b, c] = [0, 1, 2].map(|i| mesh.vertices[triangle[i] as usize]);
                let [pa, pb, pc] = [a, b, c].map(|v| Vec3::from(v.position));
                let face = (pb - pa).cross(pc - pa);
                // The cone's tip folds its last triangles to nothing.
                if face.length_squared() < 1e-12 {
                    continue;
                }
                let normal = Vec3::from(a.normal) + Vec3::from(b.normal) + Vec3::from(c.normal);
                assert!(
                    face.dot(normal) > 0.0,
                    "{primitive:?} triangle {triangle:?}"
                );
            }
        }
    }

    #[test]
    fn no_primitive_shows_its_texture_mirrored_from_outside() {
        for primitive in ALL {
            let mesh = Mesh::primitive(primitive);
            for triangle in mesh.indices.chunks(3) {
                let [a, b, c] = [0, 1, 2].map(|i| mesh.vertices[triangle[i] as usize]);
                let [pa, pb, pc] = [a, b, c].map(|v| Vec3::from(v.position));
                if (pb - pa).cross(pc - pa).length_squared() < 1e-12 {
                    continue;
                }
                // Counter-clockwise from outside, so clockwise in texture space, where V runs down.
                let [ua, ub, uc] = [a, b, c].map(|v| glam::Vec2::from(v.uv));
                assert!(
                    (ub - ua).perp_dot(uc - ua) < 0.0,
                    "{primitive:?} triangle {triangle:?}"
                );
            }
        }
    }

    #[test]
    fn the_sphere_wraps_its_texture_from_minus_z_and_pole_to_pole() {
        let mesh = Mesh::primitive(Primitive::Sphere);
        let at = |u: f32, v: f32| {
            let vertex = mesh
                .vertices
                .iter()
                .find(|vertex| vertex.uv == [u, v])
                .expect("a vertex");
            Vec3::from(vertex.position) * 2.0
        };
        let near = |a: Vec3, b: Vec3| (a - b).length() < 1e-5;
        assert!(near(at(0.0, 0.5), Vec3::NEG_Z), "the seam is at -Z");
        assert!(near(at(1.0, 0.5), Vec3::NEG_Z), "the seam is at -Z");
        assert!(near(at(0.25, 0.5), Vec3::NEG_X), "then -X");
        assert!(near(at(0.5, 0.5), Vec3::Z), "the middle faces +Z");
        assert!(near(at(0.75, 0.0), Vec3::Y), "the top row is the top pole");
        assert!(
            near(at(0.75, 1.0), Vec3::NEG_Y),
            "the bottom row the bottom pole"
        );
    }

    #[test]
    fn the_quad_shows_a_raster_upright_and_faces_forward() {
        let mesh = Mesh::textured_quad();
        let corner = |x: f32, y: f32| {
            let vertex = mesh
                .vertices
                .iter()
                .find(|vertex| vertex.position[..2] == [x, y])
                .expect("a corner");
            assert_eq!(vertex.normal, [0.0, 0.0, 1.0]);
            vertex.uv
        };
        assert_eq!(corner(-0.5, 0.5), [0.0, 0.0], "top left");
        assert_eq!(corner(0.5, 0.5), [1.0, 0.0], "top right");
        assert_eq!(corner(-0.5, -0.5), [0.0, 1.0], "bottom left");
        assert_eq!(corner(0.5, -0.5), [1.0, 1.0], "bottom right");
        let [a, b, c] =
            [0, 1, 2].map(|i| Vec3::from(mesh.vertices[mesh.indices[i] as usize].position));
        assert!((b - a).cross(c - a).z > 0.0, "counter-clockwise from +Z");
    }

    #[test]
    fn a_plane_is_as_wide_as_asked_and_as_tall_as_its_raster() {
        assert_eq!(plane_scale(2.0, (1920, 1080)), Vec3::new(2.0, 1.125, 1.0));
        assert_eq!(plane_scale(2.0, (100, 400)), Vec3::new(2.0, 8.0, 1.0));
        assert_eq!(plane_scale(2.0, (0, 0)), Vec3::new(2.0, 0.0, 1.0));
    }
}
