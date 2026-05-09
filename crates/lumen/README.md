# lumen

`lumen` is the core composition and render orchestration crate. It defines the composition model, node system, media abstractions, JSON support, and the high-level GPU renderer used by the native server, local tools, and WASM bindings.

This crate is experimental. Public APIs, node definitions, schema shapes, and render behavior may change.

## Platform Notes

The main renderer paths require a GPU through `lumen-gpu`/`wgpu`.

Supported native render targets today are:

- Linux with Vulkan, plus optional CUDA/NVENC interop when built with `vulkan,cuda,ffmpeg`.
- macOS with Metal, plus VideoToolbox-oriented media paths when built with `metal,ffmpeg`.

Browser/WASM preview paths are separate and use WebGPU/WebGL-facing code.

## Features

- `ffmpeg`: enable FFmpeg-backed media handling through `ffmpeg-next` and `lumen-ffmpeg`.
- `image`: enable image decoding helpers.
- `json`: enable JSON composition parsing and serialization helpers.
- `embed-roboto`: embed the default Roboto font assets.
- `webgl`: enable WebGL-related code paths.
- `metal`: enable Metal support through `lumen-gpu` and optional FFmpeg interop.
- `vulkan`: enable Vulkan support through `lumen-gpu` and optional FFmpeg interop.
- `cuda`: enable CUDA support through optional FFmpeg interop.

## Development

```bash
cargo check -p lumen
cargo test -p lumen
cargo check -p lumen --features ffmpeg,image,json
```
