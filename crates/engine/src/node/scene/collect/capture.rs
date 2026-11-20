//! What a scene reports of itself to a host that asks for captures, and the camera a host may
//! draw it through instead.

use std::collections::HashSet;

use lumen_3d::glam::Mat4;
use lumen_3d::{Bounds, Camera, Projection};

use super::{Collected, CopySource};
use crate::gpu::{CameraProjection, CapturedCamera, CapturedObject, SceneBounds, SceneCamera};

/// The camera `camera` gives, held to the limits `camera_3d` keeps its params to.
pub(crate) fn override_camera(camera: &SceneCamera) -> Camera {
    let (projection, near, far) = match camera.projection {
        CameraProjection::Perspective {
            fov_y_degrees,
            near,
            far,
        } => (
            Projection::Perspective {
                fov_y: fov_y_degrees.clamp(1.0, 179.0).to_radians(),
            },
            near,
            far,
        ),
        CameraProjection::Orthographic { height, near, far } => (
            Projection::Orthographic {
                height: height.max(1e-4),
            },
            near,
            far,
        ),
    };
    let near = near.max(1e-4);
    Camera {
        world: Mat4::from_cols_array(&camera.world),
        projection,
        near,
        far: far.max(near + 1e-3),
    }
}

fn captured_camera(node: crate::node::NodeId, camera: &Camera) -> CapturedCamera {
    CapturedCamera {
        node,
        world: camera.world.to_cols_array(),
        projection: match camera.projection {
            Projection::Perspective { fov_y } => CameraProjection::Perspective {
                fov_y_degrees: fov_y.to_degrees(),
                near: camera.near,
                far: camera.far,
            },
            Projection::Orthographic { height } => CameraProjection::Orthographic {
                height,
                near: camera.near,
                far: camera.far,
            },
        },
    }
}

impl Collected {
    /// The objects the scene draws and the cameras it holds, as [`SceneCapture`] reports them.
    ///
    /// [`SceneCapture`]: crate::gpu::SceneCapture
    pub(crate) fn captured(&mut self) -> (Vec<CapturedObject>, Vec<CapturedCamera>) {
        self.place_bounds();
        let singles = self.single_bounds();
        let mut objects = Vec::new();
        // A box with a material on each face is drawn in a part for each, which all say the same.
        let mut seen = HashSet::new();
        let mut report = |node: crate::node::NodeId, world: Mat4, bounds: Bounds| {
            if bounds.is_empty() || !world.is_finite() {
                return;
            }
            let world = world.to_cols_array();
            let key = (
                node,
                world.map(f32::to_bits),
                [
                    bounds.min.x.to_bits(),
                    bounds.min.y.to_bits(),
                    bounds.min.z.to_bits(),
                    bounds.max.x.to_bits(),
                    bounds.max.y.to_bits(),
                    bounds.max.z.to_bits(),
                ],
            );
            if seen.insert(key) {
                objects.push(CapturedObject {
                    node,
                    world,
                    bounds: SceneBounds {
                        min: bounds.min.to_array(),
                        max: bounds.max.to_array(),
                    },
                });
            }
        };
        for (index, (object, origin)) in self.scene.objects.iter().zip(&self.origins).enumerate() {
            let Some(copies) = object.instanced else {
                report(origin.node, object.world, object.bounds);
                continue;
            };
            match &self.lists[copies.list as usize].source {
                CopySource::Placements(placements) => {
                    let first = copies.first as usize;
                    let placed = &placements.matrices()[first..first + copies.count as usize];
                    for placement in placed {
                        let placement = copies.parent * *placement;
                        report(
                            origin.node,
                            placement * object.world,
                            singles[index].transformed(&placement),
                        );
                    }
                }
                CopySource::Particles(_) => {
                    report(origin.node, copies.parent * object.world, object.bounds);
                }
            }
        }
        let cameras = self
            .cameras
            .iter()
            .map(|(node, camera)| captured_camera(*node, camera))
            .collect();
        (objects, cameras)
    }
}
