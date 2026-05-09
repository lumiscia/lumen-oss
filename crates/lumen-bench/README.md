# lumen-bench

`lumen-bench` contains benchmark binaries for Lumen composition rendering and media decode paths.

This crate is experimental. Benchmarks are used to track renderer development and may change with internal architecture.

## Platform Notes

Benchmarks exercise GPU render paths and require GPU-capable machines for meaningful results.

Supported native targets today are:

- Linux with Vulkan, plus CUDA/NVENC benchmark modes when built with `vulkan,cuda`.
- macOS with Metal-oriented paths when built with `metal`.

Linux-only benchmark modes such as `vk-cuda-export` and `vk-cuda-nvenc` require Linux plus both `cuda` and `vulkan` features.

## Binaries

- `lumen-bench-composition`: benchmark composition render and encode modes.
- `lumen-bench-decode`: benchmark media decode paths.

## Development

```bash
cargo check -p lumen-bench
cargo check -p lumen-bench --features vulkan,cuda
cargo run -p lumen-bench --bin lumen-bench-composition -- --help
```
