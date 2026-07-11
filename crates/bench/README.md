# lumen-bench

`lumen-bench` contains benchmark binaries for Lumen composition rendering, JSON parsing, media decode, and text paths.

This crate is experimental. Benchmarks track renderer development and may change with internal architecture.

## Layout

- `src/bench/mod.rs`: `Bench` and `CompositionFixture` traits, shared timing helpers.
- `src/bench/composition/`: GPU composition render/encode modes and in-process benchmark compositions.
- `src/bench/fixtures.rs`: JSON fixtures used only by the parse benchmark (not render workloads).
- `src/bench/json_parse.rs`: JSON parse + validate timing.
- `src/bench/decode.rs`, `src/bench/text.rs`: media decode and text benchmarks.

Render benchmarks build compositions in Rust under `src/bench/composition/compositions/` instead of loading `crates/local/demo/*.json`.

## Platform Notes

Benchmarks exercise GPU render paths and require GPU-capable machines for meaningful results.

Supported native targets today are:

- Linux with Vulkan, plus CUDA/NVENC benchmark modes when built with `vulkan,cuda`.
- macOS with Metal-oriented paths when built with `metal`.

Linux-only benchmark modes such as `vk-cuda-export` and `vk-cuda-nvenc` require Linux plus both `cuda` and `vulkan` features.

## Binaries

- `lumen-bench-composition`: benchmark composition render and encode modes.
- `lumen-bench-json-parse`: benchmark JSON composition parse and validate (uses demo JSON fixtures).
- `lumen-bench-decode`: benchmark media decode paths.
- `lumen-bench-text`: benchmark text layout, raster atlas, GPU hybrid atlas, color emoji, raw glyph generation, and expression-driven text measurement paths.

Text measurement cases compare repeated `text_width` and `text_height` evaluation for literal arguments (`measure-literal`), expression-backed text-node properties (`measure-expression`), and nonrecursive nested measurement with property references (`measure-nested-reference`).

Composition names: `simple_pipeline`, `small_media_transform`, `vector_showcase`, `animated_showcase`, `antialiasing_stress_aa`, `antialiasing_stress_noaa`.

`small_media_transform` isolates the cost of transforming a native 320×180 media texture into a 1920×1080 composition canvas. Its checkerboard pixels are generated deterministically in memory, so the workload has no file or decoder dependency. To compare transform canvas-bound changes, run the same command and frame count on the base and candidate revisions:

```bash
cargo run --release -p lumen-bench --bin lumen-bench-composition -- \
  --composition small_media_transform --mode render-only --frames 120
```

Use `--mode render-profile` for the bind/upload/submit/poll breakdown. Compare the reported `elapsed_ms` and `fps` on the same machine after a warm-up run.

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

Bench output includes per-phase timings (`phase=... ms=... us=...`) for setup, mode execution, and JSON parse loops.
