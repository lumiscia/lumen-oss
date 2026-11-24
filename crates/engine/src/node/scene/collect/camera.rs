//! Finds the camera a scene is viewed from. `render_3d` and `wgsl_shader` pick it by the same
//! rule: the first camera reached walking the scene depth first in input order, placed by the
//! `merge_3d` and `instance_3d` nodes above it.

use lumen_3d::glam::Mat4;
use lumen_3d::model::{ModelScene, Pose};
use lumen_3d::{Camera, Lens, MAX_SCENE_NODES, Projection};

use super::instance::MAX_NESTING;
use super::{
    held_frames, model_error, model_playback, model_seconds, open_model, placement, pose, vec3,
};
use crate::error::{At, RenderError};
use crate::gpu::frame_graph::FrameCx;
use crate::node::scene::{
    Camera3d, Camera3dParams, Model3d, Model3dParams, Projection3d, aperture_blades_error,
};
use crate::node::{
    DelegateEvalContext, DelegateEvaluable, JsonNode, NodeId, NodeKind, required_input,
};

/// The camera the `camera_3d` `node` with `params` gives, placed by `parent`.
pub(super) fn camera_3d(
    node: NodeId,
    params: &Camera3dParams,
    parent: Mat4,
) -> crate::Result<Camera> {
    if let Some(details) = aperture_blades_error(params.aperture_blades) {
        return Err(RenderError::evaluating::<Camera3d>(node, details).into());
    }
    let local = if params.look_at {
        lumen_3d::look_at(vec3(params.pose.position), vec3(params.target))
    } else {
        pose(&params.pose).matrix()
    };
    let near = (params.near as f32).max(1e-4);
    let projection = match params.projection {
        Projection3d::Perspective => Projection::Perspective {
            fov_y: (params.fov.clamp(1.0, 179.0) as f32).to_radians(),
        },
        Projection3d::Orthographic => Projection::Orthographic {
            height: (params.ortho_height as f32).max(1e-4),
        },
    };
    let aperture = match projection {
        Projection::Perspective { fov_y } if params.f_stop > 0.0 => {
            lumen_3d::focal_length(fov_y) / params.f_stop as f32
        }
        _ => 0.0,
    };
    Ok(Camera {
        world: parent * local,
        projection,
        near,
        far: (params.far as f32).max(near + 1e-3),
        lens: Lens {
            aperture: if aperture.is_finite() { aperture } else { 0.0 },
            focus_distance: finite_or(params.focus_distance, Lens::PINHOLE.focus_distance as f64)
                .max(1e-3) as f32,
            blades: params.aperture_blades as u32,
            rotation: finite_or(params.aperture_rotation, 0.0)
                .rem_euclid(360.0)
                .to_radians() as f32,
        },
    })
}

/// `value`, or `fallback` when it is NaN or infinite.
fn finite_or(value: f64, fallback: f64) -> f64 {
    if value.is_finite() { value } else { fallback }
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
        lens: Lens::PINHOLE,
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
                    return Ok(Some(camera_3d(
                        node_id,
                        &camera.params.eval(&ctx)?,
                        parent,
                    )?));
                }
                NodeKind::Merge3d(merge) => {
                    if !merge.visible(&expr)? {
                        continue;
                    }
                    let world = parent * placement(&merge.params.placement.eval(&ctx)?).matrix();
                    let objects = merge.objects.iter().rev();
                    pending.extend(objects.map(|object| (object.node, world)));
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
                    if let Some(camera) = self.find(cx, input.node, nesting + 1)? {
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
                NodeKind::Clip(clip) => {
                    let found = cx.walk_clip(clip, |cx, output| self.find(cx, output, nesting))?;
                    if let Some(camera) = found.flatten() {
                        return Ok(Some(Camera {
                            world: parent * camera.world,
                            ..camera
                        }));
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
        if let Some(clip) = clip
            && node.animates()
        {
            cx.held_on(held_frames(
                scene.clip_span(clip),
                params,
                f64::from(cx.expr().fps),
                cx.frame_time().value(),
            ));
        }
        let seconds = model_seconds(&cx.expr());
        let pose = scene.pose(model_playback(clip, params, seconds));
        let root = parent * placement(&params.placement).matrix();
        Ok(model_camera(scene, &pose, camera, root))
    }
}
