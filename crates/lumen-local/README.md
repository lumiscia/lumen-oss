# lumen-local

`lumen-local` is a local render and debugging CLI for Lumen compositions. It is useful for testing render paths outside the hosted platform.

This crate is experimental and not intended to be the production deployment story.

## Platform Notes

Rendering requires a GPU.

Supported targets today are:

- Linux with Vulkan, plus optional CUDA/NVENC interop when built with `vulkan,cuda`.
- macOS with Metal and VideoToolbox GPU texture encode paths when built with `metal`.

VideoToolbox GPU texture encode is only available on macOS. Vulkan/CUDA export and NVENC paths are Linux-focused.

## Features

- `vulkan`: enables Vulkan renderer and FFmpeg interop features.
- `cuda`: enables CUDA/NVENC-related features.
- `metal`: enables macOS Metal and VideoToolbox-oriented paths.

## Development

```bash
cargo check -p lumen-local
cargo check -p lumen-local --features vulkan,cuda
cargo check -p lumen-local --features metal
```
