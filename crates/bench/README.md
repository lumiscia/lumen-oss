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

The text benchmark also has controlled workload scenarios:

| scenario          | workload                                           | purpose                                   |
| ----------------- | -------------------------------------------------- | ----------------------------------------- |
| `baseline`        | 64 px, short text, 1080p target                    | low-cost reference and historical default |
| `large`           | 240 px, repeated text, 1080p target                | large-glyph generation and atlas pressure |
| `dense`           | 96 px, 32 repeats, 4K target                       | high glyph count and throughput scaling   |
| `subpixel-motion` | 160 px text moving through fractional origins      | frame-to-frame positioning stability      |
| `glyph-churn`     | 128 px text with a changing frame label, 4K target | cache growth and changing-glyph stability |

`--scenario all` runs the scaling matrix. `--iterations` is the number of frames, so use at least the intended sequence length when investigating growth that isolated snapshots can hide. `--font-size`, `--text-repeats`, and `--atlas-size` override a scenario for focused sweeps.

The `raster` and experimental `gpu-msdf` cases measure CPU layout plus atlas/job preparation. They do not time GPU compute dispatch or drawing. GPU correctness and dispatch behavior belong in the `lumen-engine-text` GPU tests and composition benchmarks. The experimental MSDF case allows an MSDF pixel budget equal to the selected atlas area, preventing large scenarios from silently measuring mostly raster fallback. Its output reports MSDF jobs and pixels so fallback or saturation remains visible.

For every scenario the machine-readable line reports elapsed throughput, the minimum and maximum laid-out and rendered glyph counts, maximum host-side working bytes, used atlas bytes, MSDF jobs/pixels, and process peak RSS. A gap between `laid_out_*` and `rendered_*` signals atlas/capacity loss. Working bytes are the live output vectors for one frame; peak RSS includes the process, dependencies, and caches and is a high-water mark. Run memory comparisons as separate processes on the same machine because `ru_maxrss` never decreases within a process.

Composition names: `simple_pipeline`, `small_media_transform`, `small_media_transform_exposure`, `vector_showcase`, `animated_showcase`, `antialiasing_stress_aa`, `antialiasing_stress_noaa`.

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

Use `--mode render-profile` for the bind/upload/submit/poll breakdown. Compare the reported `elapsed_ms` and `fps` on the same machine after a warm-up run.

## Development

```bash
cargo check -p lumen-bench
cargo check -p lumen-bench --features vulkan,cuda
cargo run -p lumen-bench --bin lumen-bench-composition -- --list
cargo run -p lumen-bench --bin lumen-bench-json-parse -- --list
cargo run -p lumen-bench --bin lumen-bench-text -- --iterations 20
cargo run --release -p lumen-bench --bin lumen-bench-text -- --case raster --scenario all --iterations 120
cargo run --release -p lumen-bench --features experimental-msdf --bin lumen-bench-text -- --case gpu-msdf --scenario all --iterations 120
cargo run -p lumen-bench --bin lumen-bench-text -- --case measure-literal --iterations 100
cargo run -p lumen-bench --bin lumen-bench-text -- --case measure-expression --iterations 100
cargo run -p lumen-bench --bin lumen-bench-text -- --case measure-nested-reference --iterations 100
```

Bench output includes per-phase timings (`phase=... ms=... us=...`) for setup, mode execution, and JSON parse loops.

## Text benchmark methodology

For comparisons, build with `--release`, run one warm-up process, and collect at least three measured processes per case/scenario. Keep the revision, host, power mode, Rust toolchain, backend/features, atlas configuration, and frame count with the results. Compare medians for throughput and preserve the full glyph/resource counters; a faster result that drops glyphs is not an improvement. Use 120 frames for quick iteration and at least 1,800 frames for churn and motion soak tests.

A production-readiness evaluation for experimental MSDF should include all five scenarios at 1× and 2× atlas sizes, explicit 32/64/128/240 px font sweeps, 1080p and 4K composition renders, GPU readback fixtures, and a long sequence. This harness covers CPU preparation, capacity, allocation footprint, and sequence stability; it must be paired with GPU timestamps/readback and visual comparisons before drawing end-to-end conclusions.
