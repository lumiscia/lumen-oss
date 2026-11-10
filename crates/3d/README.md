# lumen-engine-3d

`lumen-engine-3d` holds the parts of Lumen's 3D scene nodes that need no GPU: primitive meshes, binary glTF models read from untrusted bytes and posed by their animations, transforms, cameras and their projections, lights, and the order and batches a scene's objects draw in. `lumen-engine` renders with it behind its `3d` feature.

This crate is experimental and tracks the renderer's needs rather than offering a standalone 3D API. World units are metres, Y is up and cameras look down -Z.

## Reading models

`Model::from_glb` is safe and bounded on any input: every length, index and count is checked against a limit before it is used or allocated for, and a bad file is an error rather than a panic. What it reads from a valid file is correct, and every number it keeps is finite. It is not a glTF validator, and accepts some files the specification forbids where they can still be read safely, such as bytes past the declared length, nodes no scene reaches that form a cycle, repeated key times, and two animation channels that target the same property, of which the later wins.

PNG and JPEG images are decoded by the `image` crate once their headers' sizes are within the limits, and a PNG reaches the decoder with only the chunks of its still image. A decoder that panics fails the model where panics unwind; on targets that abort on panic, such as `wasm32-unknown-unknown`, it aborts the process.

## Development

```bash
cargo check -p lumen-engine-3d
cargo test -p lumen-engine-3d
```
