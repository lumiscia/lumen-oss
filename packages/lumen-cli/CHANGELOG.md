# @lumiscia/lumen-cli

## 0.6.0-beta.1

### Minor Changes

- b8fef96: **Breaking: `lumen` now has two commands, `render` and `check`, and takes the composition as a positional argument.** The `--composition` and `--check` flags are gone, with no aliases, and `--output` has a short form, `-o`.

  Before:

  ```bash
  lumen --composition composition.json --output out.mp4 --codec hevc
  lumen --composition composition.json --output frame.png --frame 30
  lumen --composition composition.json --check --format json
  ```

  After:

  ```bash
  lumen render composition.json -o out.mp4 --codec hevc
  lumen render composition.json -o frame.png --frame 30
  lumen check composition.json --format json
  ```

  Each command takes only its own flags: `check` has `--format`, `--media-root` and `--font-dir`, and rejects the video, image and `--max-text-glyphs` flags it used to ignore, while `render` has no `--format`. The other flags and the environment variables keep their names and meanings. Bare `lumen` prints the two commands.

  `lumen render -h` and `lumen check -h` now print a line per flag, grouped into general, video and image options, and `--help` a little more. The reference that the help used to print in full (encoder order, which encoders take `--crf`, `--qp` and `--preset`, sequence numbering, environment variables) is in the [CLI README](https://github.com/lumiscia/lumen-oss/tree/main/crates/cli#readme), which the help points to.

## 0.6.0-beta.0

### Minor Changes

- 60b7798: **`lumen-local` is now `lumen`, installed from npm.** `npm install -g @lumiscia/lumen-cli` installs a prebuilt `lumen` binary for Linux on x64 and arm64 (glibc 2.28 or later) and for macOS on Apple silicon. FFmpeg 9 is linked into it with `libx264`, `libx265` (8-bit and 10-bit), `libsvtav1`, `libvpx-vp9`, `libopus` and `libdav1d`, plus the Vulkan and NVENC encoders on Linux, where NVENC takes frames through CUDA, and VideoToolbox on macOS, so nothing else needs installing. Those binaries are distributed under GPL-3.0-or-later because of x264 and x265; the source stays Apache-2.0.

  **Breaking:** the crate moves from `crates/local` to `crates/cli` and is named `lumen-cli`, and its binary is `lumen`, so `cargo run -p lumen-local --` becomes `cargo run -p lumen-cli --`. Flags and environment variables are unchanged. Errors now start with `lumen failed`.

  **AV1 uses NVENC on NVIDIA.** On a Linux host with NVIDIA's encode library, `av1` now tries `av1_nvenc`, then `libsvtav1`, instead of `av1_vulkan`, which writes wrong frames on NVIDIA. Other hosts and codecs keep their order.

  **Fixed:** `hevc_nvenc` failed to open with a `crf` or a built-in preset, because `maxrate` was set as an option of the encoder and is one of the codec context. `lumen-engine-ffmpeg` also builds against FFmpeg 9 now, which removed `AVCodec::sample_fmts`.

### Patch Changes

- 0abbc00: The platform packages of the `lumen` CLI now carry the GPL-3.0 text as `LICENSE` and, in `licenses/`, the notices of every Rust crate in the binary and the license of the built-in Roboto font.
