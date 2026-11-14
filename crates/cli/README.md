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
lumen render crates/cli/demo/gradient-paints.json -o out.mp4
lumen render crates/cli/demo/gradient-paints.json -o frame.png --frame 30
lumen render crates/cli/demo/gradient-paints.json -o frames/shot_%d.png --frames 30..60
lumen render crates/cli/demo/gradient-paints.json -o clip.mp4 --frames 30..90
lumen check crates/cli/demo/gradient-paints.json
lumen preview crates/cli/demo/gradient-paints.json -o sheet.png
node composition.js | lumen render - -o clip.mp4
```

`lumen render <composition> -o <output>` renders a composition JSON file, `lumen check <composition>` says what is wrong with one without rendering it, and `lumen preview <composition> -o <output>` tiles frames sampled across it into one image. `lumen render -h`, `lumen check -h` and `lumen preview -h` list each command's flags in a line each, and `--help` says a little more; this page is the reference.

A `<composition>` of `-` is read from stdin, so a script can pipe a composition in without writing a file: `node composition.js | lumen render - -o out.mp4`. Messages and the `lumen check` summary call it `<stdin>`, and media is found under `--media-root`, which defaults to the current directory. Empty input, or stdin that is a terminal, fails with an error at once. Use `./-` for a file named `-`.

`examples/README.md` has [3D compositions](../../examples/README.md#3d) to render this way.

## `lumen render`

`-o`, or `--output`, picks what is written. `.mp4`, `.mov` (ProRes 4444) and `.webm` (VP9) are a video of the frames, with the audio cut to the same span: AAC in `.mp4` and `.mov`, Opus in `.webm`. `.png` and `.jpg` (or `.jpeg`) are one image, and an output name with `%d` is an image per frame.

`--alpha` keeps the frame's transparency as an alpha channel, in a `.mov` (`prores_ks`) or `.webm` (`libvpx-vp9`): the colour is straight, as a PNG holds it, and alpha is full range. Alpha in an `.mp4`, or with a hardware encoder, is an error rather than an opaque video. A `.webm`'s audio is Opus, which needs an FFmpeg with `libopus`; without it, a composition with audio fails before rendering.

```sh
lumen render title.json -o title.mov --alpha
lumen render title.json -o title.webm --alpha --quality high
```

`--frames START..END` renders those frames of the composition: `END` is not included, either side can be left out (`30..`, `..60`), and frame numbers are the composition's. A video starts at time 0 and holds exactly those frames, with its audio cut to the same span. A single image is the frame at `--frame` (0 by default) or at `START`. A PNG keeps transparency; a JPEG is opaque, with transparent pixels over black, and takes `--jpeg-quality` from 1 to 100 (default 90). A sequence is written as one file per frame, replacing `%d` with the frame number padded to at least four digits (`%0Nd` pads to at least `N`; both are wider when the last frame needs it), so `frames/shot_%d.png` with `--frames 28..33` writes `shot_0028.png` to `shot_0032.png`. Directories are created. A render that fails keeps the frames it had written. A range that is empty or past the timeline fails before rendering.

| Flag                    | Environment             | Meaning                                                                                                                                                                                                                                                 |
| ----------------------- | ----------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `<composition>`         |                         | Composition JSON file to render, or `-` for stdin. Required.                                                                                                                                                                                            |
| `-o`, `--output <path>` |                         | `.mp4`, `.mov`, `.webm`, `.png`, `.jpg`, or a `%d` name for a sequence. Required. A video is written beside the output and moved into place when the render succeeds, so a failed render keeps any earlier file.                                        |
| `--frames <start..end>` |                         | The frames to render, `end` not included, either side optional. Defaults to the whole timeline.                                                                                                                                                         |
| `--media-root <dir>`    |                         | Directory media sources must stay inside. Defaults to the current directory.                                                                                                                                                                            |
| `--font-dir <dir>`      | `LUMEN_FONT_DIR`        | Must exist. Fonts looked up first, then `<media-root>/assets/fonts`, then the system fonts, by the family name inside each font file. The flag wins. A family none of them has warns once and renders in the bundled font.                              |
| `--max-text-glyphs <n>` | `LUMEN_MAX_TEXT_GLYPHS` | Most glyph, stroke and decoration instances one text node may draw, from 1 to 1048576, which bounds the work its text can ask for; whitespace is not drawn. Defaults to 16384, a full 1920 × 1080 frame of 16 px type. A text over it fails the render. |
|                         | `LUMEN_HARDWARE_DECODE` | `1`, `true`, `yes` or `on` decodes video on the GPU when the hardware supports it. Off when unset.                                                                                                                                                      |
|                         | `RUST_LOG`              | Log filter. Defaults to `lumen_engine=warn,lumen_server=warn`.                                                                                                                                                                                          |

These flags are for a video output, and any of them but `--quality-config` with an image output is an error. `--quality`, `--codec`, `--bit-depth` and `--alpha` win over the quality config's defaults. A composition holds no encoding settings; without `--codec`, a `.mov` or `.webm` takes the codec its container holds.

| Flag                      | Environment            | Meaning                                                                                                                                                                                                                                       |
| ------------------------- | ---------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `--codec <codec>`         |                        | `h264` (the default), `hevc` or `av1` in `.mp4`, `prores` in `.mov`, `vp9` in `.webm`.                                                                                                                                                        |
| `--quality <name>`        |                        | Quality preset: `draft`, `standard` (the default), `high`, `lossless`, or one the quality config defines.                                                                                                                                     |
| `--quality-config <path>` | `LUMEN_QUALITY_CONFIG` | Quality config JSON, checked before rendering. See the [server README](../server/README.md#quality-config).                                                                                                                                   |
| `--bit-depth <8\|10>`     |                        | 8 (the default) or 10, for `hevc` and `av1`, encoded from the renderer's float colour. `h264` and `vp9` with alpha are always 8, and `prores` always 10. An encoder that can't write the depth is skipped, and a pinned one fails the render. |
| `--alpha [true\|false]`   |                        | `.mov` or `.webm` only. Keeps transparency as an alpha channel. `--alpha false` is the default, an opaque video.                                                                                                                              |
| `--encoder <name>`        | `LUMEN_VIDEO_ENCODER`  | Pins one encoder with no fallback. The flag wins over the variable, and only the flag is an error with an image output.                                                                                                                       |
| `--bitrate <bps\|Nk\|NM>` |                        | Target bits per second, as `14000000`, `14000k` or `14M`.                                                                                                                                                                                     |
| `--crf <n>`               |                        | Constant rate factor.                                                                                                                                                                                                                         |
| `--qp <n>`                |                        | Constant quantizer.                                                                                                                                                                                                                           |
| `--preset <name>`         |                        | The encoder's own speed preset.                                                                                                                                                                                                               |

At most one of `--bitrate`, `--crf` and `--qp` can be given. They and `--preset` replace the quality preset's settings on every encoder tried, so none of them can go with `--quality`; [Encoders](#encoders) says which encoders take which.

These flags are for an image output:

| Flag                 | Meaning                                                                       |
| -------------------- | ----------------------------------------------------------------------------- |
| `--frame <n>`        | One image only: the frame to render. Defaults to 0. Can't go with `--frames`. |
| `--jpeg-quality <n>` | `.jpg` only, a single image or a sequence. 1 to 100, default 90.              |

Progress and the render summary go to stdout. Warnings and errors go to stderr, including a `warning font.fallback` line for each text family that fell back to the bundled font.

## `lumen check`

`lumen check <composition>` reads the composition without a GPU and prints what is wrong with it. It exits 1 if the composition has errors, and 0 if it has only warnings or none.

| Flag                    | Environment      | Meaning                                                                                                                                             |
| ----------------------- | ---------------- | --------------------------------------------------------------------------------------------------------------------------------------------------- |
| `<composition>`         |                  | Composition JSON file to check, or `-` for stdin. Required.                                                                                         |
| `--format <text\|json>` |                  | `text` (the default) prints a line per problem and a count; `json` prints `{ "ok", "diagnostics" }`, as the server's `POST /renders/check` answers. |
| `--media-root <dir>`    |                  | As for `lumen render`.                                                                                                                              |
| `--font-dir <dir>`      | `LUMEN_FONT_DIR` | As for `lumen render`.                                                                                                                              |

It takes none of the render's other flags. Fonts resolve as a render would resolve them, from `--font-dir`, the media root and the system, so a family none of them has is a `font.fallback` warning. Each problem has a stable code and a JSON pointer into the composition:

```text
error node.unknown_param at /nodes/0/params/opacityy: unknown param `opacityy` on node 1
warning font.fallback at /nodes/2/params/font_family: text node 3: font family `Brand` is not available, so its text falls back to `Roboto`
composition.json: 1 errors, 1 warnings
```

The [engine README](../engine/README.md#diagnostics) lists the codes and what only a render finds.

Sources are paths under the media root. `lumen` has no asset store, so an `asset:<id>`
reference, which only resolves on the host that issued it, is an `media.not_found` error in both
`lumen check` and a render; reference the file by its path instead.

## `lumen preview`

`lumen preview <composition> -o <output>` writes a contact sheet: frames sampled across the timeline, tiled into one `.png` or `.jpg`, to see a whole composition at a glance without rendering a video.

```sh
lumen preview comp.json -o sheet.png
lumen preview comp.json -o sheet.jpg --count 20 --columns 5 --frames 30..300
```

`--count` frames are spread evenly over `--frames`, from its first frame to its last, rounding down to whole frames, so `--count 4` over `0..10` samples 0, 3, 6 and 9. A range with no more frames than `--count` shows every frame. They are laid out a row at a time, `--columns` to a row (fewer when there are fewer frames), with the last row starting at the left. Each cell has the composition's aspect ratio and is as wide as `--width` allows with 8 pixels between cells and around the edge. The sheet is opaque: transparent pixels and the space between cells are dark grey. Each frame is rendered at the composition's size and scaled down to its cell. A zero `--count` or `--columns`, a range that is empty or past the timeline, a `--width` too narrow for the columns, and a sheet that would be more than 65535 pixels tall fail before rendering.

| Flag                    | Environment             | Meaning                                                                                                                                                     |
| ----------------------- | ----------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `<composition>`         |                         | Composition JSON file to preview, or `-` for stdin. Required.                                                                                               |
| `-o`, `--output <path>` |                         | `.png` or `.jpg` (or `.jpeg`). Required. Written beside the output and moved into place when the sheet is done, so a failed preview keeps any earlier file. |
| `--frames <start..end>` |                         | The frames to sample from, `end` not included, either side optional. Defaults to the whole timeline.                                                        |
| `--count <n>`           |                         | How many frames to sample. Defaults to 12.                                                                                                                  |
| `--columns <n>`         |                         | Frames per row. Defaults to 4.                                                                                                                              |
| `--width <px>`          |                         | The sheet's width in pixels, up to 16384. Defaults to 1920.                                                                                                 |
| `--media-root <dir>`    |                         | As for `lumen render`.                                                                                                                                      |
| `--font-dir <dir>`      | `LUMEN_FONT_DIR`        | As for `lumen render`.                                                                                                                                      |
| `--max-text-glyphs <n>` | `LUMEN_MAX_TEXT_GLYPHS` | As for `lumen render`.                                                                                                                                      |

It takes none of the video or image flags of `lumen render`. Progress and a `preview complete` line with the output, the frame count, the grid and the sheet's size go to stdout, and warnings to stderr, once each.

## Platform Notes

Rendering requires a GPU.

Supported targets today are:

- Linux with Vulkan.
- macOS with Metal and VideoToolbox GPU texture encode paths when built with `metal`.

VideoToolbox GPU texture encode is only available on macOS. `h264_nvenc` and `hevc_nvenc` work on Linux, with frames uploaded from the CPU unless built with `cuda`.

## Encoders

Each codec tries its encoders in order, warning once for each that won't open. On Linux that is the Vulkan encoder, uploading frames to it if they can't stay on the GPU, then `libx264`, `libx265` or `libsvtav1` on the CPU. On a host with NVIDIA's encode library, `av1` tries `av1_nvenc` in place of `av1_vulkan`. macOS tries VideoToolbox first, then the same CPU encoders. A quality preset with no settings for an encoder skips it, so `lossless` uses `libx264`, `libx265` or `libvpx-vp9`. `--encoder` or `LUMEN_VIDEO_ENCODER` pins one encoder, which either opens or fails the render. See the [support matrix](../../docs/support-matrix.md#encoding).

`--bitrate`, `--crf`, `--qp` and `--preset` apply to every encoder tried. One that doesn't support the flag fails to open, which moves on to the next. `--preset` alone, with no rate control, targets 14 Mb/s.

| Flag        | Encoders that take it                                                                                                      |
| ----------- | -------------------------------------------------------------------------------------------------------------------------- |
| `--bitrate` | Every encoder but `prores_ks`.                                                                                             |
| `--crf`     | `libx264`, `libx265`, `libsvtav1`, `libvpx-vp9` and NVENC.                                                                 |
| `--qp`      | `libx264`, `libx265`, `libsvtav1`, NVENC, the Vulkan encoders and `prores_ks`.                                             |
| `--preset`  | `libx264` and `libx265` (`ultrafast` to `placebo`), NVENC (`p1` to `p7`), `libsvtav1` (0 to 13) and `libvpx-vp9` (0 to 5). |

## Prebuilt binaries

The npm packages hold a binary with FFmpeg 9 and these libraries linked in, pinned by [`release/vcpkg.json`](release/vcpkg.json) and built by [`release/build.sh`](release/build.sh):

- Encoders: `libx264`, `libx265` (8-bit and 10-bit), `libsvtav1`, `libvpx-vp9`, `prores_ks` and `libopus` everywhere, the Vulkan and NVENC encoders on Linux, and VideoToolbox on macOS.
- Decoders: FFmpeg's own, `libdav1d` for AV1, and Vulkan hardware decode on Linux.

The Linux binaries are built with `cuda`, so NVENC and the Vulkan encoders both take frames without a copy through the CPU. The macOS binary is built with `metal` and runs on macOS 13 or later.

`release/ports/x265` replaces vcpkg's x265 build with one that keeps its assembly in a static Linux library and links a 10-bit build into the 8-bit one.

Lumen is Apache-2.0, but FFmpeg with x264 and x265 is GPL, so the binaries are distributed under GPL-3.0-or-later, with that text as the platform package's `LICENSE` and each part's own license in its `licenses/`. `release/about.toml` and `about.hbs` are the `cargo about` setup that writes the Rust crates' notices there. The `@lumiscia/lumen-cli` launcher itself is Apache-2.0.

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
