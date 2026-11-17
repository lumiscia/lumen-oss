//! Collects an `instance_3d`: its input scene is walked once, and its objects are copied once per
//! instance, sharing the input's meshes, textures, joint matrices and morph target weights. Opaque
//! and masked objects draw all their copies at once, from a list of the copies' placements that
//! goes to the GPU as an instance buffer; blended objects and those showing a raster are copied one
//! by one, so that each sorts and picks its raster's detail where it is.

use std::sync::{Arc, Mutex, PoisonError};

use lumen_3d::glam::{Mat3, Mat4};
use lumen_3d::{
    Blending, Bounds, Camera, Instanced, MAX_SCENE_NODES, Object, Orientation, normal_matrix,
};
use lumen_gpu::TexRef;

use super::{
    Collected, Extent, GpuMesh, MAX_SCENE_JOINTS, MAX_SCENE_WEIGHTS, PosedSkin, placement,
};
use crate::error::{ExpressionError, LumenError, RenderError};
use crate::expr::{ExpressionContext, InstanceEvaluator, InstanceScope};
use crate::gpu::frame_graph::FrameCx;
use crate::node::scene::{Instance3d, MAX_INSTANCES, Placement3d, Pose3d};
use crate::node::{Deferred, DelegateEvalContext, Node, NodeId, ParamValue, required_input};

const _: () = assert!(MAX_INSTANCES as usize <= MAX_SCENE_NODES);

/// How many `instance_3d` nodes may hold one another, which bounds the recursion collecting them.
const MAX_NESTING: u32 = 8;

fn instance_error(node_id: NodeId, details: String) -> LumenError {
    RenderError::evaluating::<Instance3d>(node_id, details).into()
}

/// `error`, if it locates an expression failure, saying which instance it happened in.
fn at_instance(error: LumenError, index: u32) -> LumenError {
    match error {
        LumenError::Expression(ExpressionError::Located { at, failure }) => {
            LumenError::Expression(ExpressionError::Located {
                at,
                failure: Box::new(ExpressionError::Evaluate {
                    details: format!("instance {index}: {failure}"),
                }),
            })
        }
        error => error,
    }
}

/// Bounds of copies a list keeps, for the objects of different extents drawing from it.
const KEPT_BOUNDS: usize = 16;

/// The placements of a list of copies: upright ones, then flat ones, then mirrored ones, leaving
/// out those that draw nothing, which aren't finite or flatten more than one axis.
pub(crate) struct Placements {
    matrices: Vec<Mat4>,
    upright: u32,
    flat: u32,
    bounds: Mutex<Vec<CopiesBounds>>,
}

/// What `copies`, the first and count of a list's copies, of something covering `one` cover.
struct CopiesBounds {
    one: Bounds,
    copies: (u32, u32),
    all: Bounds,
}

/// What a list made from an input's list is made of: that list, the first and count of its
/// copies, and what places them within the input.
type ListSource = (u32, u32, u32, Mat4);

/// How a copy placed by `matrix` faces, or `None` if it draws nothing.
fn orientation(matrix: &Mat4) -> Option<Orientation> {
    if !matrix.is_finite() {
        return None;
    }
    // In f64, as `lumen_3d::mirrors` works it out.
    let determinant = Mat3::from_mat4(*matrix).as_dmat3().determinant();
    if determinant > 0.0 {
        Some(Orientation::Upright)
    } else if determinant < 0.0 {
        Some(Orientation::Mirrored)
    } else {
        normal_matrix(matrix).map(|_| Orientation::Flat)
    }
}

impl Placements {
    fn new(matrices: Vec<Mat4>) -> Self {
        let orientations: Vec<Option<Orientation>> = matrices.iter().map(orientation).collect();
        let facing = |way| orientations.iter().filter(|&&of| of == Some(way)).count() as u32;
        let (upright, flat) = (facing(Orientation::Upright), facing(Orientation::Flat));
        let matrices = if upright as usize == matrices.len() {
            matrices
        } else {
            [
                Orientation::Upright,
                Orientation::Flat,
                Orientation::Mirrored,
            ]
            .into_iter()
            .flat_map(|way| {
                matrices
                    .iter()
                    .zip(&orientations)
                    .filter(move |&(_, &of)| of == Some(way))
                    .map(|(matrix, _)| *matrix)
            })
            .collect()
        };
        Self {
            matrices,
            upright,
            flat,
            bounds: Mutex::default(),
        }
    }

    pub(crate) fn matrices(&self) -> &[Mat4] {
        &self.matrices
    }

    /// The placements facing each way that some do: where they start, how many there are, and
    /// the way they face.
    fn runs(&self) -> impl Iterator<Item = (u32, u32, Orientation)> + use<> {
        let mirrored = self.matrices.len() as u32 - self.upright - self.flat;
        [
            (0, self.upright, Orientation::Upright),
            (self.upright, self.flat, Orientation::Flat),
            (self.upright + self.flat, mirrored, Orientation::Mirrored),
        ]
        .into_iter()
        .filter(|&(_, count, _)| count > 0)
    }

    /// What `count` copies from `first` on of something covering `one` cover together.
    pub(super) fn bounds(&self, one: Bounds, (first, count): (u32, u32)) -> Bounds {
        let mut kept = self.bounds.lock().unwrap_or_else(PoisonError::into_inner);
        let copies = (first, count);
        if let Some(kept) = kept
            .iter()
            .find(|kept| kept.one == one && kept.copies == copies)
        {
            return kept.all;
        }
        let all = self.matrices[first as usize..][..count as usize]
            .iter()
            .fold(Bounds::EMPTY, |all, matrix| {
                all.union(one.transformed(matrix))
            });
        if kept.len() == KEPT_BOUNDS {
            kept.clear();
        }
        kept.push(CopiesBounds { one, copies, all });
        all
    }
}

/// A list of placements instanced objects draw their copies from.
#[derive(Clone)]
pub(crate) struct CopyList {
    /// The `instance_3d` the list is of, and which of its lists: 0 for the copies it places, and
    /// later ones for its copies of the copies that `instance_3d` nodes in its scene place.
    pub(crate) key: (NodeId, u32),
    pub(crate) placements: Arc<Placements>,
    /// Whether frames draw the same placements again, so their buffer stays on the GPU.
    pub(crate) kept: bool,
}

/// Whether the copies of `object` can draw at once: blended surfaces sort among themselves, and a
/// raster's detail is picked by where each surface showing it is.
fn draws_from_list(object: &Object<GpuMesh, TexRef>) -> bool {
    object.surface.blending != Blending::Blend && object.surface.texture.is_none()
}

/// Evaluates the matrix placing each of an `instance_3d`'s copies in its scene's space, from its
/// `position`, `rotation` and `scale` evaluated together.
struct Placer<'n> {
    node_id: NodeId,
    /// Each param, with where a failure of it is located.
    params: [(&'static str, &'n Deferred<[f64; 3]>); 3],
    /// The params that are expressions, in their order.
    expressions: InstanceEvaluator,
}

impl<'n> Placer<'n> {
    fn new(node: &'n Instance3d) -> Self {
        let given = &node.params.placement;
        let params = [
            ("placement.position", &given.pose.position),
            ("placement.rotation", &given.pose.rotation),
            ("placement.scale", &given.scale),
        ];
        let expressions =
            InstanceEvaluator::new(params.iter().filter_map(|(_, param)| match param {
                Deferred::Expr(expression) => Some(expression),
                Deferred::Value(_) => None,
            }));
        Self {
            node_id: node.id,
            params,
            expressions,
        }
    }

    /// The matrix of copy `index`, in `scoped`, the context scoped to that copy.
    fn local(&mut self, scoped: &ExpressionContext<'_>, index: u32) -> crate::Result<Mat4> {
        let mut values = [[0.0; 3]; 3];
        let mut expression = 0;
        for (value, (path, param)) in values.iter_mut().zip(self.params) {
            *value = match param {
                Deferred::Value(value) => *value,
                Deferred::Expr(_) => {
                    expression += 1;
                    self.expressions
                        .evaluate(expression - 1, scoped)
                        .and_then(<[f64; 3]>::from_expression)
                        .map_err(|error| at_instance(error.located(self.node_id, path), index))?
                }
            };
        }
        let [position, rotation, scale] = values;
        Ok(placement(&Placement3d {
            pose: Pose3d { position, rotation },
            scale,
        })
        .matrix())
    }
}

impl Collected {
    /// Adds `node`'s scene once per instance, each copy placed by `parent` and the transform its
    /// params give for it. The scene's lights and camera are added once, placed by `parent`.
    pub(super) fn add_instances(
        &mut self,
        cx: &mut FrameCx<'_>,
        node: &Instance3d,
        parent: Mat4,
    ) -> crate::Result<()> {
        let expr = cx.expr();
        let ctx = DelegateEvalContext {
            node_id: node.id,
            expr: &expr,
        };
        let count = ctx.eval_field("count", &node.params.count)?.0;
        if count == 0 {
            return Ok(());
        }
        if self.nesting >= MAX_NESTING {
            return Err(instance_error(
                node.id,
                format!("instance_3d nodes are nested more than {MAX_NESTING} deep"),
            ));
        }
        let input = required_input(node.id, "scene", node.scene)?;
        let mut inner = Collected::new(self.render_id);
        inner.reached = self.reached;
        inner.nesting = self.nesting + 1;
        inner.instance_room = self.instance_room.saturating_sub(self.drawn) / count as usize;
        inner.add(cx, input)?;
        self.reached = inner.reached;

        let copies = u64::from(count);
        let objects = self.drawn as u64 + inner.drawn as u64 * copies;
        if objects > self.instance_room as u64 {
            return Err(instance_error(
                node.id,
                format!(
                    "{count} instances of a scene of {} objects make a scene of more than \
                     {MAX_SCENE_NODES} objects{}",
                    inner.drawn,
                    if self.nesting > 0 {
                        ", copied by the instance_3d nodes holding them"
                    } else {
                        ""
                    }
                ),
            ));
        }
        if self.indices + inner.indices * copies > u64::from(u32::MAX) {
            return Err(instance_error(
                node.id,
                format!(
                    "{count} instances of a scene of {} indices make a scene with too many \
                     vertices to draw",
                    inner.indices
                ),
            ));
        }
        let joint_base = self.scene.joints.len();
        if joint_base + inner.scene.joints.len() > MAX_SCENE_JOINTS {
            return Err(instance_error(
                node.id,
                format!(
                    "the scene's skinned models have more than {MAX_SCENE_JOINTS} joints together"
                ),
            ));
        }
        self.scene.joints.extend_from_slice(&inner.scene.joints);
        let weight_base = self.scene.weights.len();
        if weight_base + inner.scene.weights.len() > MAX_SCENE_WEIGHTS {
            return Err(instance_error(
                node.id,
                format!(
                    "the scene's morphed models have more than {MAX_SCENE_WEIGHTS} morph target \
                     weights together"
                ),
            ));
        }
        self.scene.weights.extend_from_slice(&inner.scene.weights);

        // A scene with nothing to draw has nothing to place.
        if !inner.scene.objects.is_empty() {
            self.place_copies(cx, node, count, &inner, parent, (joint_base, weight_base))?;
        }
        self.drawn += inner.drawn * count as usize;
        self.indices += inner.indices * copies;
        self.rasters.extend(inner.rasters);
        self.quad = self.quad.or(inner.quad);

        for (light, node) in inner.scene.lights.into_iter().zip(inner.light_nodes) {
            self.scene.lights.push(light.placed(&parent));
            self.light_nodes.push(node);
        }
        if self.scene.camera.is_none() {
            self.scene.camera = inner.scene.camera.map(|camera| Camera {
                world: parent * camera.world,
                ..camera
            });
        }
        Ok(())
    }

    /// Adds `count` copies of `inner`, `node`'s scene, each placed by `parent` and the transform
    /// `node`'s params give for it, with their joints and morph target weights starting at `bases`
    /// in this scene's. Objects that draw from a list draw from this node's list of placements,
    /// or for the instanced objects of `inner`, from a list of this node's copies of their copies.
    /// A single copy places the objects of `inner` as a group would.
    fn place_copies(
        &mut self,
        cx: &mut FrameCx<'_>,
        node: &Instance3d,
        count: u32,
        inner: &Collected,
        parent: Mat4,
        (joint_base, weight_base): (usize, usize),
    ) -> crate::Result<()> {
        let (placements, kept) = placements(cx, node, count)?;
        let Some(&first) = placements.matrices().first() else {
            return Ok(());
        };
        let together = count > 1;
        let from_list = |object: &Object<GpuMesh, TexRef>| {
            object.instanced.is_some() || (together && draws_from_list(object))
        };
        let objects = &inner.scene.objects;
        // The input's skins as they are, for objects drawn from lists, which place them by their
        // copies.
        let carried = self.skins.len();
        if objects.iter().any(from_list) {
            self.skins.extend(
                inner
                    .skins
                    .iter()
                    .map(|skin| rebased(skin, Mat4::IDENTITY, joint_base)),
            );
        }
        // This node's own list once an object draws from it, and those made from the input's
        // lists, by the input's list and the copies of it drawn, and by what places them.
        let mut own = None;
        let mut made: Vec<(ListSource, u32)> = Vec::new();
        for (object, extent) in objects.iter().zip(&inner.extents) {
            if !from_list(object) {
                continue;
            }
            let mut object = object.clone();
            object.first_joint = object.first_joint.map(|joint| joint + joint_base as u32);
            if let Some((_, weight)) = &mut object.morph {
                *weight += weight_base as u32;
            }
            let extent = match *extent {
                Extent::Skin(skin) => Extent::Skin(carried + skin),
                local @ Extent::Local(_) => local,
            };
            let (list, parent) = match object.instanced {
                // One copy places the input's copies as a group would.
                Some(copies) if !together => {
                    let source = (copies.list, 0, 0, Mat4::IDENTITY);
                    let list = self.made_list(&mut made, source, |_| {
                        inner.lists[copies.list as usize].clone()
                    });
                    object.instanced = Some(Instanced {
                        list,
                        parent: parent * first * copies.parent,
                        ..copies
                    });
                    self.scene.objects.push(object);
                    self.extents.push(extent);
                    continue;
                }
                Some(copies) => {
                    let source = (copies.list, copies.first, copies.count, copies.parent);
                    let list = self.made_list(&mut made, source, |earlier| {
                        flattened(cx, (node.id, earlier + 1), &placements, inner, copies)
                    });
                    (list, parent)
                }
                None => {
                    let list = *own.get_or_insert_with(|| {
                        self.lists.push(CopyList {
                            key: (node.id, 0),
                            placements: placements.clone(),
                            kept,
                        });
                        self.lists.len() as u32 - 1
                    });
                    (list, parent)
                }
            };
            for (first, count, orientation) in self.lists[list as usize].placements.runs() {
                object.instanced = Some(Instanced {
                    list,
                    first,
                    count,
                    parent,
                    orientation,
                });
                self.scene.objects.push(object.clone());
                self.extents.push(extent);
            }
        }
        let copied: Vec<usize> = (0..objects.len())
            .filter(|&index| !from_list(&objects[index]))
            .collect();
        if !copied.is_empty() {
            let copies = placements.matrices().len();
            self.scene.objects.reserve(copied.len() * copies);
            self.extents.reserve(copied.len() * copies);
            self.skins.reserve(inner.skins.len() * copies);
            for local in placements.matrices() {
                self.add_copy(inner, &copied, parent * *local, (joint_base, weight_base));
            }
        }
        Ok(())
    }

    /// The index in this scene's lists of the one made from `source`: the one `made` holds, or
    /// one `make` makes, given how many `made` holds, and added.
    fn made_list(
        &mut self,
        made: &mut Vec<(ListSource, u32)>,
        source: ListSource,
        make: impl FnOnce(u32) -> CopyList,
    ) -> u32 {
        if let Some(&(_, list)) = made.iter().find(|(of, _)| *of == source) {
            return list;
        }
        self.lists.push(make(made.len() as u32));
        let list = self.lists.len() as u32 - 1;
        made.push((source, list));
        list
    }

    /// Adds a copy of the objects of `inner` that `copied` picks, with its skins, placed by
    /// `world`, its joints and morph target weights starting at `bases` in this scene's.
    fn add_copy(
        &mut self,
        inner: &Collected,
        copied: &[usize],
        world: Mat4,
        (joint_base, weight_base): (usize, usize),
    ) {
        let first_skin = self.skins.len();
        self.skins.extend(
            inner
                .skins
                .iter()
                .map(|skin| rebased(skin, world, joint_base)),
        );
        for &index in copied {
            let mut object = inner.scene.objects[index].clone();
            object.world = world * object.world;
            object.center = world.transform_point3(object.center);
            object.first_joint = object.first_joint.map(|joint| joint + joint_base as u32);
            if let Some((_, weight)) = &mut object.morph {
                *weight += weight_base as u32;
            }
            let extent = match inner.extents[index] {
                Extent::Skin(skin) => Extent::Skin(first_skin + skin),
                local @ Extent::Local(_) => local,
            };
            self.scene.objects.push(object);
            self.extents.push(extent);
        }
    }
}

/// `skin` placed by `world`, its joints starting at `joint_base` in the scene's.
fn rebased(skin: &PosedSkin, world: Mat4, joint_base: usize) -> PosedSkin {
    PosedSkin {
        first_joint: joint_base + skin.first_joint,
        root: world * skin.root,
        joint_bounds: skin.joint_bounds.clone(),
        skin: skin.skin,
        weight: skin.weight,
    }
}

/// The placements of `node`'s `count` copies: those kept from an earlier frame when its params
/// don't read the frame, or evaluated, and whether frames draw them again.
fn placements(
    cx: &mut FrameCx<'_>,
    node: &Instance3d,
    count: u32,
) -> crate::Result<(Arc<Placements>, bool)> {
    let keep = !node.is_frame_dependent();
    if keep && let Some(kept) = cx.kept_placements(node.id, count) {
        return Ok((kept, true));
    }
    let mut placer = Placer::new(node);
    let mut scoped = cx.expr();
    let mut locals = Vec::with_capacity(count as usize);
    for index in 0..count {
        scoped.instance = Some(InstanceScope { index, count });
        locals.push(placer.local(&scoped, index)?);
    }
    let placements = Arc::new(Placements::new(locals));
    if keep {
        cx.keep_placements(node.id, count, placements.clone());
    }
    Ok((placements, keep))
}

/// The list `key` of `outer`'s copies of the copies `copies` of an instanced object of `inner`
/// draws, placed by its parent within each outer copy.
fn flattened(
    cx: &mut FrameCx<'_>,
    key: (NodeId, u32),
    outer: &Arc<Placements>,
    inner: &Collected,
    copies: Instanced,
) -> CopyList {
    let list = &inner.lists[copies.list as usize].placements;
    let range = (copies.first, copies.count);
    let (placements, kept) = cx.flattened(key, outer, (copies.parent, list, range), || {
        let copied = &list.matrices()[copies.first as usize..][..copies.count as usize];
        Placements::new(
            outer
                .matrices()
                .iter()
                .flat_map(|&outer| {
                    let outer = outer * copies.parent;
                    copied.iter().map(move |&copy| outer * copy)
                })
                .collect(),
        )
    });
    CopyList {
        key,
        placements,
        kept,
    }
}
