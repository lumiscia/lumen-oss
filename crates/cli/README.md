# lumen

`lumen` is a local render and debugging CLI for Lumen compositions. It is useful for testing render paths outside the hosted platform.

This crate is experimental and not intended to be the production deployment story.

## Install

```bash
npm install -g @lumiscia/lumen-cli
lumen --version
```

[`@lumiscia/lumen-cli`](../../packages/lumen-cli) installs a prebuilt binary for Linux on x64 and arm64 (glibc 2.28 or later) and for macOS on Apple silicon. FFmpeg is linked into the binary, so there is nothing else to install; see [Prebuilt binaries](#prebuilt-binaries) for what it holds. Rendering needs a GPU, with Vulkan on Linux and Metal on macOS.

In a checkout, `cargo run -p lumen-cli --` is the same CLI built against the system's FFmpeg.

## Usage

```bash
lumen --composition crates/cli/demo/gradient-paints.json --output out.mp4
lumen --composition crates/cli/demo/gradient-paints.json --output frame.png --frame 30
lumen --composition crates/cli/demo/gradient-paints.json --output frames/shot_%d.png --frames 30..60
lumen --composition crates/cli/demo/gradient-paints.json --output clip.mp4 --frames 30..90
lumen --composition crates/cli/demo/gradient-paints.json --check
```

`examples/README.md` has [3D compositions](../../examples/README.md#3d) to render this way.

`.mp4`, `.mov` (ProRes 4444) and `.webm` (VP9) render every frame, `.png` and `.jpg` (or `.jpeg`) render one, and an output name with `%d` renders an image per frame. `--help` lists every flag.

`--alpha` keeps the frame's transparency as an alpha channel, in a `.mov` or `.webm`: the colour is straight, as a PNG holds it, and alpha is full range. Alpha in an `.mp4`, or with a hardware encoder, is an error rather than an opaque video. A `.webm`'s audio is Opus, which needs an FFmpeg with `libopus`; without it, a composition with audio fails before rendering.

```sh
lumen --composition title.json --output title.mov --alpha
lumen --composition title.json --output title.webm --alpha --quality high
```

`--frames START..END` renders those frames of the composition: `END` is not included, either side can be left out (`30..`, `..60`), and frame numbers are the composition's. A video starts at time 0 and holds exactly those frames, with its audio cut to the same span. A single image is the frame at `--frame` (0 by default) or at `START`. A PNG keeps transparency; a JPEG is opaque, with transparent pixels over black, and takes `--jpeg-quality` from 1 to 100 (default 90). A sequence is written as one file per frame, replacing `%d` with the frame number padded to at least four digits (`%0Nd` pads to at least `N`; both are wider when the last frame needs it), so `frames/shot_%d.png` with `--frames 28..33` writes `shot_0028.png` to `shot_0032.png`. Directories are created. A render that fails keeps the frames it had written. A range that is empty or past the timeline fails before rendering.

| Flag                                               | Environment             | Meaning                                                                                                                                                                                                                          |
| -------------------------------------------------- | ----------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `--composition <path>`                             |                         | Composition JSON to render. Required.                                                                                                                                                                                            |
| `--output <path>`                                  |                         | `.mp4`, `.mov`, `.webm`, `.png`, `.jpg`, or a `%d` name for a sequence. Required unless checking. A video is written beside the output and moved into place when the render succeeds, so a failed render keeps any earlier file. |
| `--check`                                          |                         | Checks the composition without rendering it. Exits 1 if it has errors, and 0 if it has only warnings or none.                                                                                                                    |
| `--format <text\|json>`                            |                         | `--check` only. `text` prints a line per problem and a count; `json` prints `{ "ok", "diagnostics" }`, as the server's `POST /renders/check` answers.                                                                            |
| `--media-root <path>`                              |                         | Directory media sources must stay inside. Defaults to the current directory.                                                                                                                                                     |
| `--font-dir <path>`                                | `LUMEN_FONT_DIR`        | Must exist. Fonts looked up first, then `<media-root>/assets/fonts`, then the system fonts, by family name. The flag wins.                                                                                                       |
| `--encoder <name>`                                 | `LUMEN_VIDEO_ENCODER`   | Video only. Pins one encoder with no fallback. The flag wins over the variable, and `--encoder` with an image output is an error.                                                                                                |
| `--quality <name>`                                 |                         | Video only. Quality preset: `draft`, `standard` (the default), `high`, `lossless`, or one the quality config defines. Wins over the composition's `render_settings.quality`.                                                     |
| `--codec <codec>`                                  |                         | Video only. `h264` (the default), `hevc` or `av1` in `.mp4`, `prores` in `.mov`, `vp9` in `.webm`. Wins over the composition's `render_settings.codec`.                                                                          |
| `--bit-depth <8\|10>`                              |                         | Video only. 8 (the default) or 10, for `hevc` and `av1`; `prores` is always 10. Wins over the composition's `render_settings.bit_depth`. An encoder that can't write 10 bits is skipped, and a pinned one fails the render.      |
| `--alpha`                                          |                         | `.mov` or `.webm` only. Keeps transparency as an alpha channel. Wins over the composition's `render_settings.alpha`.                                                                                                             |
| `--quality-config <path>`                          | `LUMEN_QUALITY_CONFIG`  | Quality config JSON, checked before rendering. See the [server README](../server/README.md#quality-config).                                                                                                                      |
| `--bitrate <bps\|Nk\|NM>`, `--crf <n>`, `--qp <n>` |                         | Video only. At most one of the three. Replaces the preset's settings on every encoder tried, so it can't go with `--quality`.                                                                                                    |
| `--preset <name>`                                  |                         | Video only. The encoder's own speed preset. Can't go with `--quality`.                                                                                                                                                           |
| `--frame <n>`                                      |                         | One image only. Defaults to 0. Can't go with `--frames`.                                                                                                                                                                         |
| `--frames <start..end>`                            |                         | The frames to render, `end` not included, either side optional. Defaults to the whole timeline.                                                                                                                                  |
| `--jpeg-quality <n>`                               |                         | `.jpg` only. 1 to 100, default 90.                                                                                                                                                                                               |
| `--max-text-glyphs <n>`                            | `LUMEN_MAX_TEXT_GLYPHS` | Most glyph, stroke and decoration instances one text node may draw, from 1 to 1048576. Defaults to 16384, a full 1920 × 1080 frame of 16 px type. A text over it fails the render.                                               |
|                                                    | `LUMEN_HARDWARE_DECODE` | `1`, `true`, `yes` or `on` decodes video on the GPU when the hardware supports it. Off when unset.                                                                                                                               |
|                                                    | `RUST_LOG`              | Log filter. Defaults to `lumen_engine=warn,lumen_server=warn`.                                                                                                                                                                   |

Progress and the render summary go to stdout. Warnings and errors go to stderr, including a `warning font.fallback` line for each text family that fell back to the bundled font.

`--check` resolves fonts as a render would, from `--font-dir`, the media root and the system, so a family none of them has is a `font.fallback` warning. Each problem has a stable code and a JSON pointer into the composition:

```text
error node.unknown_param at /nodes/0/params/opacityy: unknown param `opacityy` on node 1
warning font.fallback at /nodes/2/params/font_family: text node 3: font family `Brand` is not available, so its text falls back to `Roboto`
composition.json: 1 errors, 1 warnings
```

The [engine README](../engine/README.md#diagnostics) lists the codes and what only a render finds.

Sources are paths under the media root. `lumen` has no asset store, so an `asset:<id>`
reference, which only resolves on the host that issued it, is an `media.not_found` error in both
`--check` and a render; reference the file by its path instead.

## Platform Notes

Rendering requires a GPU.

Supported targets today are:

- Linux with Vulkan.
- macOS with Metal and VideoToolbox GPU texture encode paths when built with `metal`.

VideoToolbox GPU texture encode is only available on macOS. `h264_nvenc` and `hevc_nvenc` work on Linux, with frames uploaded from the CPU unless built with `cuda`.

## Encoders

Each codec tries its encoders in order, warning once for each that won't open. On Linux that is the Vulkan encoder, uploading frames to it if they can't stay on the GPU, then `libx264`, `libx265` or `libsvtav1` on the CPU. On a host with NVIDIA's encode library, `av1` tries `av1_nvenc` in place of `av1_vulkan`. macOS tries VideoToolbox first, then the same CPU encoders. `--encoder` or `LUMEN_VIDEO_ENCODER` pins one encoder, which either opens or fails the render. See the [support matrix](../../docs/support-matrix.md#encoding).

## Prebuilt binaries

The npm packages hold a binary with FFmpeg 9 and these libraries linked in, pinned by [`release/vcpkg.json`](release/vcpkg.json) and built by [`release/build.sh`](release/build.sh):

- Encoders: `libx264`, `libx265` (8-bit and 10-bit), `libsvtav1`, `libvpx-vp9`, `prores_ks` and `libopus` everywhere, the Vulkan and NVENC encoders on Linux, and VideoToolbox on macOS.
- Decoders: FFmpeg's own, `libdav1d` for AV1, and Vulkan hardware decode on Linux.

The Linux binaries are built with `cuda`, so NVENC and the Vulkan encoders both take frames without a copy through the CPU. The macOS binary is built with `metal` and runs on macOS 13 or later.

`release/ports/x265` replaces vcpkg's x265 build with one that keeps its assembly in a static Linux library and links a 10-bit build into the 8-bit one.

Lumen is Apache-2.0, but FFmpeg with x264 and x265 is GPL, so the binaries are distributed under GPL-3.0-or-later, with each library's license in the platform package's `licenses/`. The `@lumiscia/lumen-cli` launcher itself is Apache-2.0.

## Features

Vulkan is always built on Linux, so it has no feature.

- `metal`: enables macOS Metal and VideoToolbox GPU texture encode.
- `cuda`: renders `h264_nvenc` frames through CUDA instead of uploading them.

## Development

```bash
cargo check -p lumen-cli
cargo check -p lumen-cli --features metal
```

`release/build.sh` builds the binary the npm package ships, into `packages/lumen-cli-<platform>/bin`. It builds FFmpeg and its libraries with vcpkg first, which takes a while the first time, and needs `git`, `zip`, `unzip`, `nasm`, `pkg-config`, the autotools and `libclang`.
