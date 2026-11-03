# lumen-engine-ffmpeg

`lumen-engine-ffmpeg` is Lumen's lower-level FFmpeg integration crate. It wraps the FFmpeg C APIs needed for demuxing, decoding, encoding, muxing, and GPU video frame interop.

This crate is experimental. The API is built around Lumen's current renderer and server needs, and may change.

## Platform Notes

Supported render/media targets today are:

- Linux with Vulkan and CUDA/NVENC for GPU interop and hardware encode paths.
- macOS with Metal and VideoToolbox-oriented frame/encode paths.

CPU decode/encode helpers exist, but Lumen's production renderer is GPU-first and should be treated as requiring GPU-capable machines.

## Media input

`InputContext::open` reads local files only, through an allowlist of demuxers in `ALLOWED_INPUT_FORMATS`. A demuxer goes on the list only if a file's data stays inside that file. Formats that open other files or hosts named by the input, such as HLS, DASH, concat lists, image sequences, and SDP, are refused. `mov` data references stay off. Anything that opens media by another route, such as the `ffmpeg` CLI, must pass the same list as `format_whitelist` and `protocol_whitelist`.

`InputContext::open` also reads existing `.jpg`, `.jpeg`, and `.bmp` files with `image2`, because FFmpeg's probe picks that demuxer for most JPEGs. It sets `pattern_type=none`, so `frame%d.jpg` only ever means a file with exactly that name, and it refuses any codec other than JPEG and BMP that `image2` picks from the file's bytes. `ALLOWED_INPUT_FORMATS` leaves `image2` out: the `ffmpeg` CLI rejects `-pattern_type` on any other demuxer, so a CLI caller can't set it safely.

## Features

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
