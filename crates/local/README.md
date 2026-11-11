# lumen-local

`lumen-local` is a local render and debugging CLI for Lumen compositions. It is useful for testing render paths outside the hosted platform.

This crate is experimental and not intended to be the production deployment story.

## Usage

```bash
cargo run -p lumen-local -- --composition crates/local/demo/gradient-paints.json --output out.mp4
cargo run -p lumen-local -- --composition crates/local/demo/gradient-paints.json --output frame.png --frame 30
cargo run -p lumen-local -- --composition crates/local/demo/gradient-paints.json --output frames/shot_%d.png --frames 30..60
cargo run -p lumen-local -- --composition crates/local/demo/gradient-paints.json --output clip.mp4 --frames 30..90
cargo run -p lumen-local -- --composition crates/local/demo/gradient-paints.json --check
```

`examples/README.md` has [3D compositions](../../examples/README.md#3d) to render this way.

`.mp4` renders every frame, `.png` and `.jpg` (or `.jpeg`) render one, and an output name with `%d` renders an image per frame. `--help` lists every flag.

`--frames START..END` renders those frames of the composition: `END` is not included, either side can be left out (`30..`, `..60`), and frame numbers are the composition's. An `.mp4` starts at time 0 and holds exactly those frames, with its audio cut to the same span. A single image is the frame at `--frame` (0 by default) or at `START`. A PNG keeps transparency; a JPEG is opaque, with transparent pixels over black, and takes `--jpeg-quality` from 1 to 100 (default 90). A sequence is written as one file per frame, replacing `%d` with the frame number padded to at least four digits (`%0Nd` pads to at least `N`; both are wider when the last frame needs it), so `frames/shot_%d.png` with `--frames 28..33` writes `shot_0028.png` to `shot_0032.png`. Directories are created. A render that fails keeps the frames it had written. A range that is empty or past the timeline fails before rendering.

| Flag                                               | Environment             | Meaning                                                                                                                                                                                                           |
| -------------------------------------------------- | ----------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `--composition <path>`                             |                         | Composition JSON to render. Required.                                                                                                                                                                             |
| `--output <path>`                                  |                         | `.mp4`, `.png`, `.jpg`, or a `%d` name for a sequence. Required unless checking. An `.mp4` is written beside the output and moved into place when the render succeeds, so a failed render keeps any earlier file. |
| `--check`                                          |                         | Checks the composition without rendering it. Exits 1 if it has errors, and 0 if it has only warnings or none.                                                                                                     |
| `--format <text\|json>`                            |                         | `--check` only. `text` prints a line per problem and a count; `json` prints `{ "ok", "diagnostics" }`, as the server's `POST /renders/check` answers.                                                             |
| `--media-root <path>`                              |                         | Directory media sources must stay inside. Defaults to the current directory.                                                                                                                                      |
| `--font-dir <path>`                                | `LUMEN_FONT_DIR`        | Must exist. Fonts looked up first, then `<media-root>/assets/fonts`, then the system fonts, by family name. The flag wins.                                                                                        |
| `--encoder <name>`                                 | `LUMEN_VIDEO_ENCODER`   | `.mp4` only. Pins one encoder with no fallback. The flag wins over the variable, and `--encoder` with an image output is an error.                                                                                |
| `--quality <name>`                                 |                         | `.mp4` only. Quality preset: `draft`, `standard` (the default), `high`, `lossless`, or one the quality config defines. Wins over the composition's `render_settings.quality`.                                     |
| `--codec <codec>`                                  |                         | `.mp4` only. `h264` (the default), `hevc` or `av1`. Wins over the composition's `render_settings.codec`.                                                                                                          |
| `--bit-depth <8\|10>`                              |                         | `.mp4` only. 8 (the default) or 10, for `hevc` and `av1`. Wins over the composition's `render_settings.bit_depth`. An encoder that can't write 10 bits is skipped, and a pinned one fails the render.             |
| `--quality-config <path>`                          | `LUMEN_QUALITY_CONFIG`  | Quality config JSON, checked before rendering. See the [server README](../server/README.md#quality-config).                                                                                                       |
| `--bitrate <bps\|Nk\|NM>`, `--crf <n>`, `--qp <n>` |                         | `.mp4` only. At most one of the three. Replaces the preset's settings on every encoder tried, so it can't go with `--quality`.                                                                                    |
| `--preset <name>`                                  |                         | `.mp4` only. The encoder's own speed preset. Can't go with `--quality`.                                                                                                                                           |
| `--frame <n>`                                      |                         | One image only. Defaults to 0. Can't go with `--frames`.                                                                                                                                                          |
| `--frames <start..end>`                            |                         | The frames to render, `end` not included, either side optional. Defaults to the whole timeline.                                                                                                                   |
| `--jpeg-quality <n>`                               |                         | `.jpg` only. 1 to 100, default 90.                                                                                                                                                                                |
| `--max-text-glyphs <n>`                            | `LUMEN_MAX_TEXT_GLYPHS` | Most glyph, stroke and decoration instances one text node may draw, from 1 to 1048576. Defaults to 16384, a full 1920 × 1080 frame of 16 px type. A text over it fails the render.                                |
|                                                    | `LUMEN_HARDWARE_DECODE` | `1`, `true`, `yes` or `on` decodes video on the GPU when the hardware supports it. Off when unset.                                                                                                                |
|                                                    | `RUST_LOG`              | Log filter. Defaults to `lumen_engine=warn,lumen_server=warn`.                                                                                                                                                    |

Progress and the render summary go to stdout. Warnings and errors go to stderr, including a `warning font.fallback` line for each text family that fell back to the bundled font.

`--check` resolves fonts as a render would, from `--font-dir`, the media root and the system, so a family none of them has is a `font.fallback` warning. Each problem has a stable code and a JSON pointer into the composition:

```text
error node.unknown_param at /nodes/0/params/opacityy: unknown param `opacityy` on node 1
warning font.fallback at /nodes/2/params/font_family: text node 3: font family `Brand` is not available, so its text falls back to `Roboto`
composition.json: 1 errors, 1 warnings
```

The [engine README](../engine/README.md#diagnostics) lists the codes and what only a render finds.

Sources are paths under the media root. `lumen-local` has no asset store, so an `asset:<id>`
reference, which only resolves on the host that issued it, is an `media.not_found` error in both
`--check` and a render; reference the file by its path instead.

## Platform Notes

Rendering requires a GPU.

Supported targets today are:

- Linux with Vulkan.
- macOS with Metal and VideoToolbox GPU texture encode paths when built with `metal`.

VideoToolbox GPU texture encode is only available on macOS. `h264_nvenc` and `hevc_nvenc` work on Linux, with frames uploaded from the CPU unless built with `cuda`.

## Encoders

Each codec tries its encoders in order, warning once for each that won't open. On Linux that is the Vulkan encoder, uploading frames to it if they can't stay on the GPU, then `libx264`, `libx265` or `libsvtav1` on the CPU. macOS tries VideoToolbox first, then the same CPU encoders. `--encoder` or `LUMEN_VIDEO_ENCODER` pins one encoder, which either opens or fails the render. See the [support matrix](../../docs/support-matrix.md#encoding).

## Features

Vulkan is always built on Linux, so it has no feature.

- `metal`: enables macOS Metal and VideoToolbox GPU texture encode.
- `cuda`: renders `h264_nvenc` frames through CUDA instead of uploading them.

## Development

```bash
cargo check -p lumen-local
cargo check -p lumen-local --features metal
```
