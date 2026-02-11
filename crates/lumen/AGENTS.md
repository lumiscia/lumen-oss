# crates/lumen — Core Render Engine

Rust crate: project compiler, source pipeline, and Vello GPU renderer.

## Structure

```
src/
├── lib.rs              # Public API surface
├── model.rs            # Project/Source/Layer/Timeline data model
├── source_pipeline.rs  # Trim, speed, reverse, loop transforms
├── compile.rs          # Project → frame plan compiler
├── gpu.rs              # Vello GPU renderer (wgpu + ffmpeg encode)
└── time.rs             # Frame/time utilities
```

## Data Model

- **Project**: `sources` (reusable media/generators) + `layers` (timeline clips) + `timeline` (fps, total_frames).
- **Source pipeline**: Chainable transforms — `trim`, `speed`, `reverse`, `loop`.
- **Compiler**: Resolves a project into per-frame render plans.
- **GPU renderer**: Uses Vello for compute-driven rasterization, reads back textures, encodes with FFmpeg.

## Guardrails

- No `todo!()`, `panic!()`, `unwrap()`, or `expect()` in runtime paths. Use `Result<T, E>` with typed errors.
- Keep the GPU renderer context long-lived — don't recreate `wgpu` instances per frame.
- Render submissions must stay serialized on one thread to avoid `wgpu` context churn.
- Frame/decode caches must be bounded. Add backpressure before adding capacity.
- Local media paths must resolve against allowlisted roots only. Reject `..`, symlinks outside root, and non-file URI schemes.
- Run `cargo check --workspace` and `cargo test --no-run --workspace` after changes.

## Related Crates

- `crates/lumen-server` — Render executor, Runpod adapter binary.
- `crates/lumen-wasm` — WASM bindings for browser preview.
- `crates/lumen-local` — CLI for local rendering (`--project <path> --output <path>`).

## Commands

```bash
cargo check -p lumen
cargo test -p lumen
cargo clippy -p lumen
```
