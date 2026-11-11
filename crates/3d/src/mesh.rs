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

const SEGMENTS: u32 = 64;
const RINGS: u32 = 32;

impl Mesh {
    /// `primitive` one unit across in X and Z, centred on the origin. Round primitives stand along
    /// Y, a unit tall but for the flat plane and the torus, whose tube is a quarter unit thick.
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
                        mesh.vertex(normal * 0.5 + right * x + up * y, normal);
                    }
                    mesh.quad(base, base + 1, base + 2, base + 3);
                }
            }
            Primitive::Plane => {
                for (x, z) in [(-0.5, 0.5), (0.5, 0.5), (0.5, -0.5), (-0.5, -0.5)] {
                    mesh.vertex(Vec3::new(x, 0.0, z), Vec3::Y);
                }
                mesh.quad(0, 1, 2, 3);
            }
            Primitive::Sphere => {
                for ring in 0..=RINGS {
                    let polar = ring as f32 / RINGS as f32 * PI;
                    for segment in 0..=SEGMENTS {
                        let azimuth = segment as f32 / SEGMENTS as f32 * TAU;
                        let normal = Vec3::new(
                            -polar.sin() * azimuth.cos(),
                            polar.cos(),
                            polar.sin() * azimuth.sin(),
                        );
                        mesh.vertex(normal * 0.5, normal);
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
                for (y, radius) in [(0.5, top), (-0.5, 0.5)] {
                    for segment in 0..=SEGMENTS {
                        let (sin, cos) = (segment as f32 / SEGMENTS as f32 * TAU).sin_cos();
                        mesh.vertex(
                            Vec3::new(sin * radius, y, cos * radius),
                            Vec3::new(sin, slope, cos).normalize(),
                        );
                    }
                }
                mesh.grid(1, SEGMENTS);
                for (y, radius) in [(0.5, top), (-0.5, 0.5)] {
                    if radius == 0.0 {
                        continue;
                    }
                    let normal = Vec3::new(0.0, y * 2.0, 0.0);
                    let center = mesh.vertices.len() as u32;
                    mesh.vertex(Vec3::new(0.0, y, 0.0), normal);
                    for segment in 0..=SEGMENTS {
                        let (sin, cos) = (segment as f32 / SEGMENTS as f32 * TAU).sin_cos();
                        mesh.vertex(Vec3::new(sin * radius, y, cos * radius), normal);
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
                    // The tube angle runs down the outer equator first, so the grid winds outward.
                    let (tube_sin, tube_cos) = (-(ring as f32) / RINGS as f32 * TAU).sin_cos();
                    for segment in 0..=SEGMENTS {
                        let (sin, cos) = (segment as f32 / SEGMENTS as f32 * TAU).sin_cos();
                        let normal = Vec3::new(cos * tube_cos, tube_sin, -sin * tube_cos);
                        let center = Vec3::new(cos * major, 0.0, -sin * major);
                        mesh.vertex(center + normal * minor, normal);
                    }
                }
                mesh.grid(RINGS, SEGMENTS);
            }
        }
        mesh
    }

    fn vertex(&mut self, position: Vec3, normal: Vec3) {
        self.vertices.push(Vertex {
            position: position.into(),
            normal: normal.into(),
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
}
