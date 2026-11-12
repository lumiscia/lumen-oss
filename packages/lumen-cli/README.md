# @lumiscia/lumen-cli

The `lumen` command checks and renders [Lumen](https://github.com/lumiscia/lumen-oss) compositions on a local GPU.

```bash
npm install -g @lumiscia/lumen-cli

lumen check composition.json
lumen render composition.json -o out.mp4
lumen render composition.json -o frame.png --frame 30
```

It installs a prebuilt binary for Linux on x64 and arm64 (glibc 2.28 or later) and for macOS on Apple silicon, with FFmpeg linked in. Rendering needs a GPU, with Vulkan on Linux and Metal on macOS.

`lumen render --help` and `lumen check --help` list every flag, and the [CLI README](https://github.com/lumiscia/lumen-oss/tree/main/crates/cli#readme) explains them.

## License

This package is Apache-2.0. The binary it installs comes from a platform package such as `@lumiscia/lumen-cli-linux-x64` and contains FFmpeg with x264 and x265, so it is distributed under GPL-3.0-or-later.
