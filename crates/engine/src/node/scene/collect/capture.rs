//! What a scene reports of itself to a host that asks for captures, and the camera a host may
//! draw it through instead.

use std::collections::hash_map::Entry;

use foldhash::HashMap;
use lumen_3d::glam::Mat4;
use lumen_3d::{Bounds, Camera, Lens, Projection};

use super::{Collected, CopySource, Within};
use crate::composition::Composition;
use crate::gpu::{
    CameraLens, CameraProjection, CapturedCamera, CapturedObject, SceneBounds, SceneCamera,
};

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
        lens: Lens::PINHOLE,
    }
}

/// The template a node `within` is in, and the clips that play the templates it is in.
fn place(composition: &Composition, within: &Within) -> (Option<String>, Vec<crate::node::NodeId>) {
    (
        composition.template_name(within.scene).map(str::to_string),
        within.clips.clone(),
    )
}

fn captured_camera(
    composition: &Composition,
    node: crate::node::NodeId,
    within: &Within,
    camera: &Camera,
) -> CapturedCamera {
    let (template, clips) = place(composition, within);
    CapturedCamera {
        node,
        template,
        clips,
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
        lens: CameraLens {
            aperture: camera.lens.aperture,
            focus_distance: camera.lens.focus_distance,
            blades: camera.lens.blades,
            rotation_degrees: camera.lens.rotation.to_degrees(),
        },
    }
}

impl Collected {
    /// The objects the scene draws and the cameras it holds, as [`SceneCapture`] reports them,
    /// with the templates of `composition` they are in.
    ///
    /// [`SceneCapture`]: crate::gpu::SceneCapture
    pub(crate) fn captured(
        &mut self,
        composition: &Composition,
    ) -> (Vec<CapturedObject>, Vec<CapturedCamera>) {
        self.place_bounds();
        let singles = self.single_bounds();
        let mut found: Vec<(crate::node::NodeId, &Within, Mat4, Bounds)> = Vec::new();
        // Where in `found` each particle system's entry is, by the list that places its particles.
        let mut systems: HashMap<u32, usize> = HashMap::default();
        for (index, (object, origin)) in self.scene.objects.iter().zip(&self.origins).enumerate() {
            // The other parts of an object a node draws in several say what the first does.
            if origin.extra_part {
                continue;
            }
            let Some(copies) = object.instanced else {
                found.push((origin.node, &origin.within, object.world, object.bounds));
                continue;
            };
            let list = &self.lists[copies.list as usize];
            match &list.source {
                CopySource::Placements(placements) => {
                    let first = copies.first as usize;
                    let placed = &placements.matrices()[first..first + copies.count as usize];
                    for placement in placed {
                        let placement = copies.parent * *placement;
                        found.push((
                            origin.node,
                            &origin.within,
                            placement * object.world,
                            singles[index].transformed(&placement),
                        ));
                    }
                }
                CopySource::Particles(_) => match systems.entry(copies.list) {
                    Entry::Occupied(system) => {
                        let (.., bounds) = &mut found[*system.get()];
                        *bounds = bounds.union(object.bounds);
                    }
                    Entry::Vacant(system) => {
                        system.insert(found.len());
                        found.push((list.key.0, &origin.within, copies.parent, object.bounds));
                    }
                },
            }
        }
        let objects = found
            .into_iter()
            .filter(|(.., world, bounds)| !bounds.is_empty() && world.is_finite())
            .map(|(node, within, world, bounds)| {
                let (template, clips) = place(composition, within);
                CapturedObject {
                    node,
                    template,
                    clips,
                    world: world.to_cols_array(),
                    bounds: SceneBounds {
                        min: bounds.min.to_array(),
                        max: bounds.max.to_array(),
                    },
                }
            })
            .collect();
        let cameras = self
            .cameras
            .iter()
            .map(|(node, within, camera)| captured_camera(composition, *node, within, camera))
            .collect();
        (objects, cameras)
    }
}
