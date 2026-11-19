//! Finds the camera a scene is viewed from. `render_3d` and `wgsl_shader` pick it by the same
//! rule: the first camera reached walking the scene depth first in input order, placed by the
//! `merge_3d` and `instance_3d` nodes above it.

use lumen_3d::glam::Mat4;
use lumen_3d::model::{ModelScene, Pose};
use lumen_3d::{Camera, MAX_SCENE_NODES, Projection};

use super::instance::MAX_NESTING;
use super::{model_error, model_playback, model_seconds, open_model, placement, pose, vec3};
use crate::error::{At, RenderError};
use crate::gpu::frame_graph::FrameCx;
use crate::node::scene::{Camera3dParams, Model3d, Model3dParams, Projection3d};
use crate::node::{
    DelegateEvalContext, DelegateEvaluable, JsonNode, NodeId, NodeKind, required_input,
};

/// The camera a `camera_3d` with `params` gives, placed by `parent`.
pub(super) fn camera_3d(params: &Camera3dParams, parent: Mat4) -> Camera {
    let local = if params.look_at {
        lumen_3d::look_at(vec3(params.pose.position), vec3(params.target))
    } else {
        pose(&params.pose).matrix()
    };
    let near = (params.near as f32).max(1e-4);
    Camera {
        world: parent * local,
        projection: match params.projection {
            Projection3d::Perspective => Projection::Perspective {
                fov_y: (params.fov.clamp(1.0, 179.0) as f32).to_radians(),
            },
            Projection3d::Orthographic => Projection::Orthographic {
                height: (params.ortho_height as f32).max(1e-4),
            },
        },
        near,
        far: (params.far as f32).max(near + 1e-3),
    }
}

/// The camera at index `camera` of a model in `pose`, for a model placed by `root`, if the pose
/// places it.
pub(super) fn model_camera(
    scene: &ModelScene,
    pose: &Pose,
    camera: usize,
    root: Mat4,
) -> Option<Camera> {
    let &(_, world) = pose.cameras.iter().find(|(index, _)| *index == camera)?;
    let model_camera = scene.cameras().get(camera)?;
    // A camera keeps its node's place and turn, but not its scale.
    let (_, rotation, translation) = (root * world).to_scale_rotation_translation();
    let world = Mat4::from_rotation_translation(rotation, translation);
    world.is_finite().then_some(Camera {
        world,
        projection: model_camera.projection,
        near: model_camera.near,
        far: model_camera.far,
    })
}

/// The camera `render_3d` views the scene `scene` from, without collecting what it draws; `None`
/// if the scene has none, which `render_3d` views with [`Camera::default`]. A scene reaching more
/// than [`MAX_SCENE_NODES`] nodes, counting a node each time it is reached, fails as a node `N`,
/// which `requester` is.
pub(crate) fn find_camera<N: JsonNode>(
    cx: &mut FrameCx<'_>,
    requester: NodeId,
    scene: NodeId,
) -> crate::Result<Option<Camera>> {
    Search::<N> {
        requester,
        reached: 0,
        kind: std::marker::PhantomData,
    }
    .find(cx, scene, 0)
}

struct Search<N> {
    requester: NodeId,
    reached: usize,
    kind: std::marker::PhantomData<N>,
}

impl<N: JsonNode> Search<N> {
    fn find(
        &mut self,
        cx: &mut FrameCx<'_>,
        scene: NodeId,
        nesting: u32,
    ) -> crate::Result<Option<Camera>> {
        // Each node still to walk, with the transform its parents place it by.
        let mut pending = vec![(scene, Mat4::IDENTITY)];
        while let Some((node_id, parent)) = pending.pop() {
            self.reached += 1;
            if self.reached > MAX_SCENE_NODES {
                return Err(RenderError::evaluating::<N>(
                    self.requester,
                    format!(
                        "the scene expands to more than {MAX_SCENE_NODES} nodes, counting a \
                         node each time it is reached"
                    ),
                )
                .into());
            }
            let node = cx.node(node_id).ok_or(RenderError::MissingNode {
                at: At::node(node_id),
            })?;
            let expr = cx.expr();
            let ctx = DelegateEvalContext::new(node_id, &expr);
            match node {
                NodeKind::Camera3d(camera) => {
                    return Ok(Some(camera_3d(&camera.params.eval(&ctx)?, parent)));
                }
                NodeKind::Merge3d(merge) => {
                    let world = parent * placement(&merge.params.eval(&ctx)?.placement).matrix();
                    pending.extend(merge.objects.iter().rev().map(|&object| (object, world)));
                }
                NodeKind::Instance3d(instance) => {
                    if ctx.eval_field("count", &instance.params.count)?.0 == 0 {
                        continue;
                    }
                    if nesting >= MAX_NESTING {
                        return Err(RenderError::evaluating::<N>(
                            self.requester,
                            format!("instance_3d nodes are nested more than {MAX_NESTING} deep"),
                        )
                        .into());
                    }
                    let input = required_input(instance.id, "scene", instance.scene)?;
                    // The instanced scene is searched as a scene of its own, and its camera is
                    // then placed by the parents, as a collect places it.
                    if let Some(camera) = self.find(cx, input, nesting + 1)? {
                        return Ok(Some(Camera {
                            world: parent * camera.world,
                            ..camera
                        }));
                    }
                }
                NodeKind::Model3d(model) => {
                    let params = model.params.eval(&ctx)?;
                    if params.camera.is_empty() {
                        continue;
                    }
                    if let Some(camera) = Self::model_camera(cx, model, &params, parent)? {
                        return Ok(Some(camera));
                    }
                }
                _ => {}
            }
        }
        Ok(None)
    }

    /// The camera `params` select in `node`'s model, posed at the frame, if the model places it.
    fn model_camera(
        cx: &mut FrameCx<'_>,
        node: &Model3d,
        params: &Model3dParams,
        parent: Mat4,
    ) -> crate::Result<Option<Camera>> {
        let source = params.source.as_str();
        let error = |details: String| model_error(node.id, source, details);
        let model = open_model(cx, node.id, source)?;
        let scene = &model.scene;
        let Some(camera) = scene.camera(&params.camera).map_err(error)? else {
            return Ok(None);
        };
        let clip = scene.animation(&params.animation).map_err(error)?;
        let seconds = model_seconds(&cx.expr());
        let pose = scene.pose(model_playback(clip, params, seconds));
        let root = parent * placement(&params.placement).matrix();
        Ok(model_camera(scene, &pose, camera, root))
    }
}
