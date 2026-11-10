use lumen_3d::model::{Model, Playback};

/// Reads `bytes` as a model and, when it reads, poses it at rest and through its first clips,
/// panicking on anything non-finite.
pub fn check(bytes: &[u8]) {
    let Ok(model) = Model::from_glb(bytes) else {
        return;
    };
    for vertex in &model.geometry.vertices {
        assert!(
            vertex
                .position
                .iter()
                .chain(&vertex.normal)
                .chain(&vertex.uv)
                .all(|v| v.is_finite())
        );
    }
    let mut playbacks = vec![None];
    for clip in 0..3 {
        for seconds in [0.0, 0.37, 1.5, 1e4] {
            for looped in [false, true] {
                playbacks.push(Some(Playback {
                    clip,
                    seconds,
                    looped,
                }));
            }
        }
    }
    for playback in playbacks {
        let pose = model.scene.pose(playback);
        assert!(pose.primitives.iter().all(|posed| posed.world.is_finite()));
        assert!(pose.joints.iter().all(|joint| joint.is_finite()));
    }
}
