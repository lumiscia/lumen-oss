# lumen-local

`lumen-local` is a local render and debugging CLI for Lumen compositions. It is useful for testing render paths outside the hosted platform.

This crate is experimental and not intended to be the production deployment story.

## Usage

```bash
cargo run -p lumen-local -- --composition crates/local/demo/gradient-paints.json --output out.mp4
cargo run -p lumen-local -- --composition crates/local/demo/gradient-paints.json --output frame.png --frame 30
cargo run -p lumen-local -- --composition crates/local/demo/gradient-paints.json --check
```

`.mp4` renders every frame, and `.png` renders one. `--help` lists every flag.

| Flag                                               | Environment             | Meaning                                                                                                                                                                       |
| -------------------------------------------------- | ----------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `--composition <path>`                             |                         | Composition JSON to render. Required.                                                                                                                                         |
| `--output <path>`                                  |                         | `.mp4` or `.png`. Required unless checking. An `.mp4` is written beside the output and moved into place when the render succeeds, so a failed render keeps any earlier file.  |
| `--check`                                          |                         | Checks the composition without rendering it. Exits 1 if it has errors, and 0 if it has only warnings or none.                                                                 |
| `--format <text\|json>`                            |                         | `--check` only. `text` prints a line per problem and a count; `json` prints `{ "ok", "diagnostics" }`, as the server's `POST /renders/check` answers.                         |
| `--media-root <path>`                              |                         | Directory media sources must stay inside. Defaults to the current directory.                                                                                                  |
| `--font-dir <path>`                                | `LUMEN_FONT_DIR`        | Must exist. Fonts looked up first, then `<media-root>/assets/fonts`, then the system fonts, by family name. The flag wins.                                                    |
| `--encoder <name>`                                 | `LUMEN_VIDEO_ENCODER`   | `.mp4` only. Pins one encoder with no fallback. The flag wins over the variable, and `--encoder` with a `.png` output is an error.                                            |
| `--quality <name>`                                 |                         | `.mp4` only. Quality preset: `draft`, `standard` (the default), `high`, `lossless`, or one the quality config defines. Wins over the composition's `render_settings.quality`. |
| `--codec <codec>`                                  |                         | `.mp4` only. `h264` (the default), `hevc` or `av1`. Wins over the composition's `render_settings.codec`.                                                                      |
| `--quality-config <path>`                          | `LUMEN_QUALITY_CONFIG`  | Quality config JSON, checked before rendering. See the [server README](../server/README.md#quality-config).                                                                   |
| `--bitrate <bps\|Nk\|NM>`, `--crf <n>`, `--qp <n>` |                         | `.mp4` only. At most one of the three. Replaces the preset's settings on every encoder tried, so it can't go with `--quality`.                                                |
| `--preset <name>`                                  |                         | `.mp4` only. The encoder's own speed preset. Can't go with `--quality`.                                                                                                       |
| `--frame <n>`                                      |                         | `.png` only. Defaults to 0.                                                                                                                                                   |
|                                                    | `LUMEN_HARDWARE_DECODE` | `1`, `true`, `yes` or `on` decodes video on the GPU when the hardware supports it. Off when unset.                                                                            |
|                                                    | `RUST_LOG`              | Log filter. Defaults to `lumen_engine=warn,lumen_server=warn`.                                                                                                                |

Progress and the render summary go to stdout. Warnings and errors go to stderr, including a `warning font.fallback` line for each text family that fell back to the bundled font.

`--check` resolves fonts as a render would, from `--font-dir`, the media root and the system, so a family none of them has is a `font.fallback` warning. Each problem has a stable code and a JSON pointer into the composition:

```text
error node.unknown_param at /nodes/0/params/opacityy: unknown param `opacityy` on node 1
warning font.fallback at /nodes/2/params/font_family: text node 3: font family `Brand` is not available, so its text falls back to `Roboto`
composition.json: 1 errors, 1 warnings
```

The [engine README](../engine/README.md#diagnostics) lists the codes and what only a render finds.

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
