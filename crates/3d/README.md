# lumen-engine-3d

`lumen-engine-3d` holds the parts of Lumen's 3D scene nodes that need no GPU: primitive meshes, binary glTF models read from untrusted bytes and posed by their animations, transforms, cameras and their projections, lights, and the order and batches a scene's objects draw in. `lumen-engine` renders with it behind its `3d` feature.

This crate is experimental and tracks the renderer's needs rather than offering a standalone 3D API. World units are metres, Y is up and cameras look down -Z.

## Development

```bash
cargo check -p lumen-engine-3d
cargo test -p lumen-engine-3d
```
