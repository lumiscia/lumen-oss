---
"@lumiscia/lumen-cli": minor
"@lumiscia/lumen-bindings": minor
---

**`lumen-local` is now `lumen`, installed from npm.** `npm install -g @lumiscia/lumen-cli` installs a prebuilt `lumen` binary for Linux on x64 and arm64 (glibc 2.28 or later) and for macOS on Apple silicon. FFmpeg 9 is linked into it with `libx264`, `libx265` (8-bit and 10-bit), `libsvtav1`, `libvpx-vp9`, `libopus` and `libdav1d`, plus the Vulkan and NVENC encoders on Linux, where NVENC takes frames through CUDA, and VideoToolbox on macOS, so nothing else needs installing. Those binaries are distributed under GPL-3.0-or-later because of x264 and x265; the source stays Apache-2.0.

**Breaking:** the crate moves from `crates/local` to `crates/cli` and is named `lumen-cli`, and its binary is `lumen`, so `cargo run -p lumen-local --` becomes `cargo run -p lumen-cli --`. Flags and environment variables are unchanged. Errors now start with `lumen failed`.

**AV1 uses NVENC on NVIDIA.** On a Linux host with NVIDIA's encode library, `av1` now tries `av1_nvenc`, then `libsvtav1`, instead of `av1_vulkan`, which writes wrong frames on NVIDIA. Other hosts and codecs keep their order.

**Fixed:** `hevc_nvenc` failed to open with a `crf` or a built-in preset, because `maxrate` was set as an option of the encoder and is one of the codec context. `lumen-engine-ffmpeg` also builds against FFmpeg 9 now, which removed `AVCodec::sample_fmts`.
