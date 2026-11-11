---
"@lumiscia/lumen-cli": minor
"@lumiscia/lumen-bindings": minor
---

**`lumen-local` is now `lumen`, installed from npm.** `npm install -g @lumiscia/lumen-cli` installs a prebuilt `lumen` binary for Linux on x64 and arm64 (glibc 2.28 or later) and for macOS on Apple silicon. FFmpeg 9 is linked into it with `libx264`, `libx265`, `libsvtav1`, `libvpx-vp9`, `libopus` and `libdav1d`, plus the Vulkan and NVENC encoders on Linux and VideoToolbox on macOS, so nothing else needs installing. Those binaries are distributed under GPL-3.0-or-later because of x264 and x265; the source stays Apache-2.0.

**Breaking:** the crate moves from `crates/local` to `crates/cli` and is named `lumen-cli`, and its binary is `lumen`, so `cargo run -p lumen-local --` becomes `cargo run -p lumen-cli --`. Flags and environment variables are unchanged. Errors now start with `lumen failed`.
