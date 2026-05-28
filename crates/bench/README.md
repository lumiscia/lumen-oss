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
- `lumen-bench-text`: benchmark text layout, raster atlas, GPU hybrid atlas, color emoji, and raw glyph generation paths.

Composition names: `simple_pipeline`, `vector_showcase`, `animated_showcase`, `antialiasing_stress_aa`, `antialiasing_stress_noaa`.

## Development

```bash
cargo check -p lumen-bench
cargo check -p lumen-bench --features vulkan,cuda
cargo run -p lumen-bench --bin lumen-bench-composition -- --list
cargo run -p lumen-bench --bin lumen-bench-json-parse -- --list
cargo run -p lumen-bench --bin lumen-bench-text -- --iterations 20
```

Bench output includes per-phase timings (`phase=... ms=... us=...`) for setup, mode execution, and JSON parse loops.
