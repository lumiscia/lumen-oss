# lumen-bench

`lumen-bench` contains benchmark binaries for Lumen composition rendering, JSON parsing, media decode, and text paths.

This crate is experimental. Benchmarks track renderer development and may change with internal architecture.

## Layout

- `src/bench/mod.rs`: the `CompositionFixture` trait; each binary calls its module's `run()`.
- `src/bench/composition/`: GPU composition render/encode modes and in-process benchmark compositions.
- `src/bench/fixtures.rs`: JSON fixtures used only by the parse benchmark (not render workloads).
- `src/bench/json_parse.rs`: JSON parse + validate timing.
- `src/bench/decode.rs`, `src/bench/text.rs`: media decode and text benchmarks.

Render benchmarks build compositions in Rust under `src/bench/composition/compositions/` instead of loading `crates/local/demo/*.json`.

## Platform Notes

Benchmarks exercise GPU render paths and require GPU-capable machines for meaningful results.

Supported native targets today are:

- Linux with Vulkan, plus the `vulkan-video` mode when built with `vulkan` and the CUDA/NVENC mode when built with `vulkan,cuda`.
- macOS with Metal-oriented paths when built with `metal`.

`vulkan-video` renders and encodes on one Vulkan device and uses the first of H.264, HEVC and AV1 that the machine can encode; the chosen encoder is printed. `vk-cuda-nvenc` requires Linux plus both `cuda` and `vulkan` features. Modes a build or platform lacks are reported as skipped.

## Binaries

- `lumen-bench-composition`: benchmark composition render and encode modes: `render-only`, `readback`, `cpu-encode`, `metal-videotoolbox`, `vulkan-video` and `vk-cuda-nvenc`. `--profile` adds a per-phase breakdown to any of them.
- `lumen-bench-json-parse`: benchmark JSON composition parse and validate (uses demo JSON fixtures).
- `lumen-bench-decode`: compare in-process decoding with the ffmpeg CLI. The CLI figure is wall clock for the whole process, including start-up and the rawvideo pipe, so the comparison is end-to-end. The run fails if the two paths decode different amounts of video.
- `lumen-bench-text`: benchmark text layout, production raster/MSDF rendering (`--case raster` / `--case msdf`), emoji fallback, and expression-driven text measurement. Rendering varies font size each frame with bidi, combining marks, and wrapping.

The `emoji` case is skipped when no loaded font covers the emoji. The `measure-*` cases run a fixed workload and reject `--font` and `--text-repeats`.

Text measurement cases compare repeated `text_width` and `text_height` evaluation for literal arguments (`measure-literal`), expression-backed text-node properties (`measure-expression`), and nonrecursive nested measurement with property references (`measure-nested-reference`).

Composition names: `simple_pipeline`, `small_media_transform`, `small_media_transform_exposure`, `animated_showcase`, `antialiasing_stress_aa`, `antialiasing_stress_noaa`, `many_elements`.

`many_elements` is 300 labelled cards and 100 dots over a 1080p background, each card a shape and a text label merged into a group. `render-only` also reports `plan_texture_bytes`, the GPU memory the compiled plan's textures take.

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

Each row reports `setup_ms` (renderer, plan compile and encoder open), `first_frame_ms` (one-off uploads, static work and glyph population) and `steady_ms` over the remaining frames on the same renderer, and `fps` is the steady-state rate, `-` for a single frame. `elapsed_ms` is the first frame plus the steady frames. Encoders buffer frames, so no frame is complete until the encoder finishes: encoding modes report `first_frame_ms=-`, and `steady_ms` and `fps` cover every frame through finishing the encoder. Plan diagnostics from `--profile` and the plan-memory line are printed outside every timer.

Nodes without expressions or media keep their output after the first frame, so `simple_pipeline` runs almost no passes after it. Add `--full-render` to disable that automatic reuse of static node output when measuring shader or pass cost; memos whose key is unchanged still hold. The antialiasing pair always renders in full, since it exists to compare edge shader cost, and every row prints the `full_render` policy it ran with. Compare `fps` on the same machine; short runs are noisy on GPUs that change clocks.

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
