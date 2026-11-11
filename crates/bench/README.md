# lumen-bench

`lumen-bench` contains benchmark binaries for Lumen composition rendering, JSON parsing, media decode, and text paths.

This crate is experimental. Benchmarks track renderer development and may change with internal architecture.

## Layout

- `src/bench/mod.rs`: the `CompositionFixture` trait; each binary calls its module's `run()`.
- `src/bench/composition/`: GPU composition render/encode modes and in-process benchmark compositions.
- `src/bench/fixtures.rs`: JSON fixtures used only by the parse benchmark (not render workloads).
- `src/bench/json_parse.rs`: JSON parse + validate timing.
- `src/bench/decode.rs`, `src/bench/expression.rs`, `src/bench/text.rs`: media decode, expression and text benchmarks.

Render benchmarks build compositions in Rust under `src/bench/composition/compositions/` instead of loading `crates/local/demo/*.json`.

`lumen-bench-frame-graph` reads fonts, and the `launch` composition, from a directory you pass with `--launch-dir` (it holds `fonts/` and `launch-video.json`). `launch`, `dynamic_text`, `dynamic_path`, `motion_blur`, `shader_switch` and `file:<path>` fail with a clear error without it. `remapped_media` and the named in-process compositions don't need it. `minified_image` (a 3840x2160 image under a transform that animates it to about 480x270, its source kept across frames), `minified_video` (the same with a 3840x2160 video, whose source changes every frame) `shrunk_video` (a 1920x1080 video at about 0.75), `half_video` (the same at about 0.5) and `unscaled_video` (the same at scale 1) and `video_scale:<s>` (the same at scale `s`) measure the filtering of a shrunk source and what it costs one that isn't, and need no directory either. A `file:<path>` composition can also show the in-memory 1920x1080 `video` source that `remapped_media` plays.

## Platform Notes

Benchmarks exercise GPU render paths and require GPU-capable machines for meaningful results.

Supported native targets today are:

- Linux with Vulkan, plus the `vulkan-video` mode when built with `vulkan` and the CUDA/NVENC mode when built with `vulkan,cuda`.
- macOS with Metal-oriented paths when built with `metal`.

`vulkan-video` renders and encodes on one Vulkan device and uses the first of H.264, HEVC and AV1 that the machine can encode; the chosen encoder is printed. `vk-cuda-nvenc` requires Linux plus both `cuda` and `vulkan` features. Modes a build or platform lacks are reported as skipped.

## Binaries

- `lumen-bench-composition`: benchmark composition render and encode modes: `render-only`, `readback`, `cpu-encode`, `metal-videotoolbox`, `vulkan-video` and `vk-cuda-nvenc`. `--profile` adds a per-phase breakdown to any of them.
- `lumen-bench-frame-graph`: benchmark rendering a composition, one run per process: time per frame, allocations and held memory.
- `lumen-bench-json-parse`: benchmark JSON composition parse and validate (uses demo JSON fixtures).
- `lumen-bench-decode`: compare in-process decoding with the ffmpeg CLI. The CLI figure is wall clock for the whole process, including start-up and the rawvideo pipe, so the comparison is end-to-end. The run fails if the two paths decode different amounts of video.
- `lumen-bench-expression`: time parsing and evaluating representative expressions, with no GPU or media.
- `lumen-bench-3d` (needs `--features 3d`): `render_3d` on scenes built in-process, so nothing is downloaded or checked in. `heavy` flies a camera over a generated terrain model of `--triangles` (default 320000) triangles in 64 × 64 tiles, each with its own material, at 720p and 1080p with shadows and an environment map on and off; `plane` shows a 4096² image plane smaller than its source, unchanged and changing, and the same image on a model's material; `clip-end` plays a model whose clip has ended. Each row splits the milliseconds per frame into recording (including scene collection), upload, submit and waiting for the GPU, and prints the passes recorded per frame. `--scenario NAME` (repeatable), `--frames N` and `--supersample N` narrow the run. To measure a machine, such as one with an NVIDIA GPU, run `cargo run --release -p lumen-bench --features 3d --bin lumen-bench-3d` on it.
- `lumen-bench-text`: benchmark text layout, production raster/MSDF rendering (`--case raster` / `--case msdf`), emoji fallback, and expression-driven text measurement. Rendering varies font size each frame with bidi, combining marks, and wrapping.

The `emoji` case is skipped when no loaded font covers the emoji. The `measure-*` cases run a fixed workload and reject `--font` and `--text-repeats`.

Text measurement cases compare repeated `text_width` and `text_height` evaluation for literal arguments (`measure-literal`), expression-backed text-node properties (`measure-expression`), and nonrecursive nested measurement with property references (`measure-nested-reference`).

Composition names: `simple_pipeline`, `small_media_transform`, `small_media_transform_exposure`, `animated_showcase`, `antialiasing_stress_aa`, `antialiasing_stress_noaa`, `many_elements`.

`many_elements` is 300 labelled cards and 100 dots over a 1080p background, each card a shape and a text label merged into a group. `render-only` also prints a `render_profile` line with the passes recorded, the programs built and the GPU memory held.

The small-media fixtures use the same deterministic in-memory 320×180 checkerboard and 1920×1080 composition canvas, so they have no file or decoder dependency:

- `small_media_transform` measures `Media → Transform → Output`.
- `small_media_transform_exposure` measures `Media → Transform → Exposure → Output`, exposing the cost of a downstream unary filter after the transform expands to canvas bounds.

To compare transform canvas-bound changes, run both workloads with the same command and frame count on the base and candidate revisions:

```bash
for composition in small_media_transform small_media_transform_exposure; do
  cargo run --release -p lumen-bench --bin lumen-bench-composition -- \
    --composition "$composition" --mode render-only --frames 1200
done
```

Use `--profile` for the bind/upload/submit/poll breakdown.

Each row reports `setup_ms` (renderer, program builds and encoder open), and `elapsed_ms`, the first frame plus the steady frames. In `render-only` and `readback`, `first_frame_ms` is one-off uploads, static work and glyph population, `steady_ms` covers the remaining frames on the same renderer, and `fps` is their steady-state rate, `-` for a single frame. Encoders buffer frames, so no frame is complete until the encoder finishes: encoding modes report `first_frame_ms=-`, and `steady_ms` and `fps` cover every frame through finishing the encoder, including with a single frame. Diagnostics from `--profile` and the held-memory line are printed outside every timer.

Nodes without expressions or media keep their output after the first frame, so `simple_pipeline` runs almost no passes after it. Add `--full-render` to disable that automatic reuse of static node output when measuring shader or pass cost. The antialiasing pair always renders in full, since it exists to compare edge shader cost, and every row prints the `full_render` policy it ran with. Compare `fps` on the same machine; short runs are noisy on GPUs that change clocks.

With `--save PATH`, a run that selects exactly one encoding output writes to `PATH`. Otherwise `PATH` is a prefix and each file is named `<stem>_<composition>_<mode>.<ext>`.

## Development

```bash
cargo check -p lumen-bench
cargo check -p lumen-bench --features vulkan,cuda
cargo run -p lumen-bench --bin lumen-bench-composition -- --list
cargo run -p lumen-bench --bin lumen-bench-json-parse -- --list
cargo run -p lumen-bench --bin lumen-bench-text -- --iterations 20
cargo run -p lumen-bench --bin lumen-bench-text -- --case measure-literal --iterations 100
cargo run -p lumen-bench --bin lumen-bench-text -- --case measure-expression --iterations 100
cargo run -p lumen-bench --bin lumen-bench-text -- --case measure-nested-reference --iterations 100
```

Count options reject zero and negative values.
