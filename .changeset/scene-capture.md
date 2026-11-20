---
"@lumiscia/lumen-bindings": minor
---

Hosts that build an editor on the Rust engine can ask where each `render_3d` places its scene on a frame, and draw it from a camera of their own without editing the composition. `GpuCompositionRenderer::set_scene_capture` makes every recorded frame report a `SceneCapture` per `render_3d`, with each drawn object's node, world matrix and world bounds, and every camera the scene holds, the first being the one it renders through. `set_camera_override` draws every `render_3d` through a given camera instead, with no recompile. While either is on, no output is kept from earlier frames, so each frame is placed again.

```rust
renderer.set_scene_capture(true);
renderer.set_camera_override(Some(SceneCamera {
    world: look_at_matrix, // camera to world, 16 floats, column-major
    projection: CameraProjection::Perspective { fov_y_degrees: 50.0, near: 0.1, far: 100.0 },
}));
let frame = renderer.render(&composition, 0.0, &media, RenderTarget::Owned)?;
for capture in renderer.take_scene_captures() {
    for object in capture.objects {
        println!("{:?} spans {:?}..{:?}", object.node, object.bounds.min, object.bounds.max);
    }
}
```
