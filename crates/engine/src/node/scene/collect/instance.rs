//! Collects an `instance_3d`: its input scene is walked once, and its objects are copied once per
//! instance, sharing the input's meshes, textures, joint matrices and morph target weights.

use lumen_3d::glam::Mat4;
use lumen_3d::{Camera, MAX_SCENE_NODES};

use super::{Collected, Extent, MAX_SCENE_JOINTS, MAX_SCENE_WEIGHTS, PosedSkin, placement};
use crate::error::{ExpressionError, LumenError, RenderError};
use crate::expr::{ExpressionContext, InstanceEvaluator, InstanceScope};
use crate::gpu::frame_graph::FrameCx;
use crate::node::scene::{Instance3d, MAX_INSTANCES, Placement3d, Pose3d};
use crate::node::{Deferred, DelegateEvalContext, Node, NodeId, ParamValue, required_input};

const _: () = assert!(MAX_INSTANCES as usize <= MAX_SCENE_NODES);

/// How many `instance_3d` nodes may hold one another, which bounds the recursion collecting them.
const MAX_NESTING: u32 = 8;

fn instance_error(node_id: NodeId, details: String) -> LumenError {
    RenderError::NodeEvaluation {
        node_id,
        node_kind: "Instance3d",
        details,
    }
    .into()
}

/// `error`, if it locates an expression failure, saying which instance it happened in.
fn at_instance(error: LumenError, index: u32) -> LumenError {
    match error {
        LumenError::Expression(ExpressionError::Located {
            node_id,
            path,
            failure,
        }) => LumenError::Expression(ExpressionError::Located {
            node_id,
            path,
            failure: Box::new(ExpressionError::Evaluate {
                details: format!("instance {index}: {failure}"),
            }),
        }),
        error => error,
    }
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
        inner.instance_room =
            self.instance_room.saturating_sub(self.scene.objects.len()) / count as usize;
        inner.add(cx, input)?;
        self.reached = inner.reached;

        let copies = u64::from(count);
        let objects = self.scene.objects.len() as u64 + inner.scene.objects.len() as u64 * copies;
        if objects > self.instance_room as u64 {
            return Err(instance_error(
                node.id,
                format!(
                    "{count} instances of a scene of {} objects make a scene of more than \
                     {MAX_SCENE_NODES} objects{}",
                    inner.scene.objects.len(),
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
        self.scene
            .objects
            .reserve(inner.scene.objects.len() * count as usize);
        self.extents
            .reserve(inner.scene.objects.len() * count as usize);
        self.skins.reserve(inner.skins.len() * count as usize);

        // A scene with nothing to draw has nothing to place.
        if !inner.scene.objects.is_empty() {
            self.place_copies(cx, node, count, &inner, parent, (joint_base, weight_base))?;
        }
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
    /// in this scene's. Params that don't read the frame place the copies the same way on every
    /// frame, so their matrices are kept.
    fn place_copies(
        &mut self,
        cx: &mut FrameCx<'_>,
        node: &Instance3d,
        count: u32,
        inner: &Collected,
        parent: Mat4,
        (joint_base, weight_base): (usize, usize),
    ) -> crate::Result<()> {
        let keep = !node.is_frame_dependent();
        if keep && let Some(locals) = cx.kept_placements(node.id, count) {
            for local in locals.iter() {
                self.add_copy(inner, parent * *local, joint_base, weight_base);
            }
            return Ok(());
        }
        let mut locals = Vec::with_capacity(if keep { count as usize } else { 0 });
        let mut placer = Placer::new(node);
        let mut scoped = cx.expr();
        for index in 0..count {
            scoped.instance = Some(InstanceScope { index, count });
            let local = placer.local(&scoped, index)?;
            if keep {
                locals.push(local);
            }
            self.add_copy(inner, parent * local, joint_base, weight_base);
        }
        if keep {
            cx.keep_placements(node.id, count, locals.into());
        }
        Ok(())
    }

    /// Adds a copy of `inner`'s objects and skins placed by `world`, its joints and morph target
    /// weights starting at `joint_base` and `weight_base` in this scene's.
    fn add_copy(&mut self, inner: &Collected, world: Mat4, joint_base: usize, weight_base: usize) {
        let first_skin = self.skins.len();
        self.skins.extend(inner.skins.iter().map(|skin| PosedSkin {
            first_joint: joint_base + skin.first_joint,
            root: world * skin.root,
            joint_bounds: skin.joint_bounds.clone(),
            skin: skin.skin,
            weight: skin.weight,
        }));
        for (object, extent) in inner.scene.objects.iter().zip(&inner.extents) {
            let mut object = object.clone();
            object.world = world * object.world;
            object.center = world.transform_point3(object.center);
            object.first_joint = object.first_joint.map(|joint| joint + joint_base as u32);
            if let Some((_, weight)) = &mut object.morph {
                *weight += weight_base as u32;
            }
            let extent = match *extent {
                Extent::Skin(skin) => Extent::Skin(first_skin + skin),
                Extent::Local(bounds) => Extent::Local(bounds),
            };
            self.scene.objects.push(object);
            self.extents.push(extent);
        }
    }
}
