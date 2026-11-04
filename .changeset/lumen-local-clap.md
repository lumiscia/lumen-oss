---
"@lumiscia/lumen-bindings": minor
---

`lumen-local` parses its command line with clap, so `--help` prints to stdout, a bad command line exits with code 2 and clap's message, and `--encoder` or `--bitrate 14M` with a `.png` output is an error even when the value matches the default. `LUMEN_VIDEO_ENCODER` and `LUMEN_FONT_DIR` still act as defaults for the flags. The `vulkan` feature of `lumen-local` is gone, because Vulkan is always built on Linux.
