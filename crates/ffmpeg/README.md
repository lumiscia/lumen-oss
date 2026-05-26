# lumen-engine-ffmpeg

`lumen-engine-ffmpeg` is Lumen's lower-level FFmpeg integration crate. It wraps the FFmpeg C APIs needed for demuxing, decoding, encoding, muxing, and GPU video frame interop.

This crate is experimental. The API is built around Lumen's current renderer and server needs, and may change.

## Platform Notes

Supported render/media targets today are:

- Linux with Vulkan and CUDA/NVENC for GPU interop and hardware encode paths.
- macOS with Metal and VideoToolbox-oriented frame/encode paths.

CPU decode/encode helpers exist, but Lumen's production renderer is GPU-first and should be treated as requiring GPU-capable machines.

## Features

- `avcodec`, `avformat`, `swresample`, `swscale`: FFmpeg component features enabled by default.
- `metal`: enables CoreVideo/Metal interop types used on macOS.
- `vulkan`: enables Vulkan video frame interop types.
- `cuda`: enables CUDA interop via `cudarc` and NVENC helpers. Kernel sources live under `crates/ffmpeg/cuda/` and are compiled to PTX with `nvcc` at build time (requires the CUDA toolkit on Linux). Override the target with `LUMEN_CUDA_ARCH` (default `sm_50`) or the compiler with `CUDA_NVCC`.
- `build-ffmpeg`: asks `ffmpeg-sys-next` to build FFmpeg.
- `gpl`: enables GPL FFmpeg build licensing through `ffmpeg-sys-next`.
- `nonfree`: enables nonfree FFmpeg build licensing through `ffmpeg-sys-next`.

## Development

```bash
cargo check -p lumen-engine-ffmpeg
cargo test -p lumen-engine-ffmpeg
cargo check -p lumen-engine-ffmpeg --features vulkan,cuda
```
