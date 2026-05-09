# lumen-gpu

`lumen-gpu` contains the lower-level GPU rendering infrastructure used by `lumen`: renderer setup, texture and buffer resources, render/compute program descriptions, pass planning, and backend-specific interop helpers.

This crate is experimental and closely tracks the renderer internals.

## Platform Notes

Lumen's native rendering requires a GPU.

Supported native backends today are:

- Vulkan on Linux.
- Metal on macOS.

The crate uses `wgpu` as its primary GPU abstraction. Vulkan export helpers are only compiled on Linux with the `vulkan` feature. macOS dependencies enable Metal support for the platform target.

## Features

- `metal`: enables explicit Metal backend support through `wgpu`/`wgpu-hal`.
- `vulkan`: enables Vulkan backend support and Vulkan export helpers.

## Development

```bash
cargo check -p lumen-gpu
cargo check -p lumen-gpu --features vulkan
cargo test -p lumen-gpu
```
