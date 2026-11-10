//! The order a scene's objects draw in, and the runs of them one draw covers: consecutive objects
//! that bind the same mesh and pipeline, laid out in one table whose rows a shader finds from the
//! vertex it is drawing.

use std::collections::HashMap;
use std::fmt;
use std::hash::Hash;

use glam::{Mat3, Mat4};

use crate::{Blending, Object, mirrors, normal_matrix};

/// The most objects one frame may draw. Whoever expands a scene into objects should stop once it
/// has reached this many of its parts, counting a part each time it is reached.
pub const MAX_OBJECTS: usize = 100_000;

/// What objects drawn together share.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DrawKey<M> {
    pub blend: bool,
    pub double_sided: bool,
    /// Whether the object's world matrix mirrors it, which turns the faces that point away.
    pub mirrored: bool,
    pub mesh: M,
}

impl<M: Copy> DrawKey<M> {
    fn of(object: &Object<M>) -> Self {
        Self {
            blend: object.surface.blending == Blending::Blend,
            double_sided: object.surface.double_sided,
            mirrored: mirrors(&object.world),
            mesh: object.mesh,
        }
    }
}

/// One row of the object table.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Row {
    /// The object's index in the scene.
    pub object: usize,
    /// Where the object's vertices start among all the table's, one per index it draws.
    pub first_vertex: u32,
    /// The object's [`normal_matrix`].
    pub normal: Mat3,
}

/// One draw: `count` rows from `first`, whose `vertices` start at `base` among all the table's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Run<M> {
    pub key: DrawKey<M>,
    pub first: u32,
    pub count: u32,
    pub base: u32,
    pub vertices: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Draws<M> {
    pub rows: Vec<Row>,
    pub runs: Vec<Run<M>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrawError {
    TooManyObjects,
    TooManyVertices,
}

impl fmt::Display for DrawError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooManyObjects => write!(f, "the scene has more than {MAX_OBJECTS} objects"),
            Self::TooManyVertices => write!(f, "the scene has too many vertices to draw"),
        }
    }
}

impl std::error::Error for DrawError {}

impl<M: Copy + Eq + Hash> Draws<M> {
    /// Opaque objects first, grouped by what they bind and otherwise in scene order, then blended
    /// ones from back to front as `view` sees them. Objects that draw nothing are left out: those
    /// without indices, and those whose world matrix has no [`normal_matrix`].
    pub fn new(objects: &[Object<M>], view: &Mat4) -> Result<Self, DrawError> {
        if objects.len() > MAX_OBJECTS {
            return Err(DrawError::TooManyObjects);
        }
        let keys: Vec<DrawKey<M>> = objects.iter().map(DrawKey::of).collect();
        let mut seen: HashMap<DrawKey<M>, usize> = HashMap::new();
        let groups: Vec<usize> = keys
            .iter()
            .map(|&key| {
                let next = seen.len();
                *seen.entry(key).or_insert(next)
            })
            .collect();
        let depth = |index: usize| view.transform_point3(objects[index].center).z;
        let mut order: Vec<(usize, Mat3)> = objects
            .iter()
            .enumerate()
            .filter(|(_, object)| object.index_count > 0)
            .filter_map(|(index, object)| Some((index, normal_matrix(&object.world)?)))
            .collect();
        order.sort_by(|&(a, _), &(b, _)| {
            keys[a].blend.cmp(&keys[b].blend).then_with(|| {
                if keys[a].blend {
                    depth(a).total_cmp(&depth(b))
                } else {
                    groups[a].cmp(&groups[b])
                }
            })
        });

        let mut draws = Self {
            rows: Vec::with_capacity(order.len()),
            runs: Vec::new(),
        };
        let mut vertices = 0u32;
        for (row, (index, normal)) in order.into_iter().enumerate() {
            let count = objects[index].index_count;
            let first_vertex = vertices;
            vertices = vertices
                .checked_add(count)
                .ok_or(DrawError::TooManyVertices)?;
            draws.rows.push(Row {
                object: index,
                first_vertex,
                normal,
            });
            match draws.runs.last_mut() {
                Some(run) if run.key == keys[index] => {
                    run.count += 1;
                    run.vertices += count;
                }
                _ => draws.runs.push(Run {
                    key: keys[index],
                    first: row as u32,
                    count: 1,
                    base: first_vertex,
                    vertices: count,
                }),
            }
        }
        Ok(draws)
    }
}

#[cfg(test)]
mod tests {
    use glam::Vec3;

    use super::*;
    use crate::Surface;

    fn object(mesh: u8, blending: Blending, z: f32) -> Object<u8> {
        Object {
            world: Mat4::from_translation(Vec3::new(0.0, 0.0, z)),
            mesh,
            index_count: 6,
            surface: Surface {
                base_color: [1.0; 4],
                metallic: 0.0,
                roughness: 0.5,
                emissive: [0.0; 3],
                blending,
                unlit: false,
                double_sided: false,
            },
            center: Vec3::new(0.0, 0.0, z),
        }
    }

    #[test]
    fn opaque_objects_group_by_mesh_and_blended_ones_follow_back_to_front() {
        let objects = [
            object(1, Blending::Blend, -1.0),
            object(1, Blending::Opaque, 0.0),
            object(2, Blending::Opaque, 0.0),
            object(1, Blending::Blend, -3.0),
            object(1, Blending::Opaque, 0.0),
            object(2, Blending::Blend, -2.0),
        ];
        let draws = Draws::new(&objects, &Mat4::IDENTITY).unwrap();
        let order: Vec<usize> = draws.rows.iter().map(|row| row.object).collect();
        assert_eq!(order, [1, 4, 2, 3, 5, 0]);
        let firsts: Vec<u32> = draws.rows.iter().map(|row| row.first_vertex).collect();
        assert_eq!(firsts, [0, 6, 12, 18, 24, 30]);
        let runs: Vec<(u8, bool, u32, u32, u32, u32)> = draws
            .runs
            .iter()
            .map(|run| {
                let Run {
                    key,
                    first,
                    count,
                    base,
                    vertices,
                } = *run;
                (key.mesh, key.blend, first, count, base, vertices)
            })
            .collect();
        assert_eq!(
            runs,
            [
                (1, false, 0, 2, 0, 12),
                (2, false, 2, 1, 12, 6),
                (1, true, 3, 1, 18, 6),
                (2, true, 4, 1, 24, 6),
                (1, true, 5, 1, 30, 6),
            ]
        );
    }

    #[test]
    fn objects_that_draw_nothing_are_left_out() {
        let mut empty = object(1, Blending::Opaque, 0.0);
        empty.index_count = 0;
        let mut flattened = object(1, Blending::Opaque, 0.0);
        flattened.world = Mat4::from_scale(Vec3::new(0.0, 0.0, 1.0));
        let objects = [empty, flattened, object(1, Blending::Opaque, 0.0)];
        let draws = Draws::new(&objects, &Mat4::IDENTITY).unwrap();
        let rows: Vec<(usize, u32)> = draws
            .rows
            .iter()
            .map(|row| (row.object, row.first_vertex))
            .collect();
        assert_eq!(rows, [(2, 0)]);
    }

    #[test]
    fn mirrored_objects_draw_apart_from_unmirrored_ones() {
        let mut mirrored = object(1, Blending::Opaque, 0.0);
        mirrored.world = Mat4::from_scale(Vec3::new(-1.0, 1.0, 1.0));
        let objects = [object(1, Blending::Opaque, 0.0), mirrored];
        let draws = Draws::new(&objects, &Mat4::IDENTITY).unwrap();
        let runs: Vec<(bool, u32)> = draws
            .runs
            .iter()
            .map(|run| (run.key.mirrored, run.count))
            .collect();
        assert_eq!(runs, [(false, 1), (true, 1)]);
    }
}
