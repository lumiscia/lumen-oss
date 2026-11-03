# lumen-local

`lumen-local` is a local render and debugging CLI for Lumen compositions. It is useful for testing render paths outside the hosted platform.

This crate is experimental and not intended to be the production deployment story.

## Usage

```bash
cargo run -p lumen-local -- --composition crates/local/demo/gradient-paints.json --output out.mp4
cargo run -p lumen-local -- --composition crates/local/demo/gradient-paints.json --output frame.png --frame 30
```

`.mp4` renders every frame, and `.png` renders one. `--help` lists every flag.

| Flag                                               | Environment             | Meaning                                                                                                                                                      |
| -------------------------------------------------- | ----------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `--composition <path>`                             |                         | Composition JSON to render. Required.                                                                                                                        |
| `--output <path>`                                  |                         | `.mp4` or `.png`. Required. An `.mp4` is written beside the output and moved into place when the render succeeds, so a failed render keeps any earlier file. |
| `--media-root <path>`                              |                         | Directory media sources must stay inside. Defaults to the current directory.                                                                                 |
| `--font-dir <path>`                                | `LUMEN_FONT_DIR`        | Must exist. Fonts looked up first, then `<media-root>/assets/fonts`, then the system fonts, by family name. The flag wins.                                   |
| `--encoder <name>`                                 | `LUMEN_VIDEO_ENCODER`   | `.mp4` only. The flag wins over the variable, and `--encoder` with a `.png` output is an error.                                                              |
| `--bitrate <bps\|Nk\|NM>`, `--crf <n>`, `--qp <n>` |                         | `.mp4` only. At most one of the three.                                                                                                                       |
| `--preset <name>`                                  |                         | `.mp4` only.                                                                                                                                                 |
| `--frame <n>`                                      |                         | `.png` only. Defaults to 0.                                                                                                                                  |
|                                                    | `LUMEN_HARDWARE_DECODE` | `1`, `true`, `yes` or `on` decodes video on the GPU when the hardware supports it. Off when unset.                                                           |
|                                                    | `RUST_LOG`              | Log filter. Defaults to `lumen_engine=warn,lumen_server=warn`.                                                                                               |

Progress and the render summary go to stdout. Warnings and errors go to stderr.

## Platform Notes

Rendering requires a GPU.

Supported targets today are:

- Linux with Vulkan.
- macOS with Metal and VideoToolbox GPU texture encode paths when built with `metal`.

VideoToolbox GPU texture encode is only available on macOS. `h264_nvenc` and `hevc_nvenc` work as ordinary encoders on Linux, with frames uploaded from the CPU.

## Encoders

`--encoder` or `LUMEN_VIDEO_ENCODER` names the encoder, and a named encoder that won't open fails the render. With neither set, Linux uses `h264_vulkan` and falls back to `libx264` on the CPU with one warning, and macOS uses `h264_videotoolbox`. See the [support matrix](../../docs/support-matrix.md#encoding).

## Features

Vulkan is always built on Linux, so it has no feature.

- `metal`: enables macOS Metal and VideoToolbox GPU texture encode.
- `cuda`: compiles the CUDA support in `lumen-engine` and `lumen-ffmpeg`. `lumen-local` does not use it.

## Development

```bash
cargo check -p lumen-local
cargo check -p lumen-local --features metal
```
