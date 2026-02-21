# crates/lumen — Core Render Engine

Rust crate: project compiler, expression engine, and Skia GPU renderer.

## Structure

```
src/
├── lib.rs                  # Public API surface
├── time.rs                 # Rational type, frame/time utilities
├── expression.rs           # Expression parser and evaluator (e.g. `canvas.width * 0.5`)
├── orchestrator.rs         # Multi-threaded frame render orchestration
├── model/
│   ├── mod.rs              # Re-exports
│   ├── project.rs          # Project, Canvas, Timeline, AudioMix
│   ├── source.rs           # Source, SourceKind, SourceMedia
│   ├── clip.rs             # Layer, LayerItem, ClipItem, VideoPipeline
│   ├── style.rs            # BaseStyle, ClipStyle, TransformStyle, StyleValue
│   └── layout.rs           # LayoutNode, LayoutNodeKind, LayoutNodeStyle
├── compile/
│   ├── mod.rs              # Project → CompiledTimeline compiler
│   ├── operation.rs        # CompiledOperation, CompiledTimeline, runtime types
│   ├── scalar.rs           # Scalar compilation (literal vs expression)
│   └── dependency.rs       # Expression dependency graph + cycle detection
└── backend/
    ├── mod.rs              # Renderer/FrameProvider traits, error types
    └── skia/               # Skia renderer backend (feature-gated: renderer-skia)
        ├── mod.rs           # SkiaRenderer impl, frame rendering, masking dispatch
        ├── primitives.rs    # Drawing: shapes, text, images, color matrices
        ├── layout.rs        # Flexbox layout computation (taffy)
        ├── mask.rs          # Mask compositing (simple clip vs DstIn)
        └── shadow.rs        # Drop shadow paint construction
```

## Data Model

- **Project**: `sources` (reusable media/generators) + `layers` (timeline clips) + `timeline` (fps, total_frames).
- **VideoPipeline**: Per-clip video transforms — `trim`, `speed`, `loop` (on `ClipItem`).
- **Expressions**: Runtime-evaluated math referencing clip/canvas properties (e.g. `canvas.width * 0.5 + clip_a.x`).
- **Compiler**: Resolves a project into a `CompiledTimeline` with per-frame render plans, scalar bindings, and dependency-ordered expression evaluation.
- **Orchestrator**: Multi-threaded frame rendering with backpressure budget and ordered output.
- **GPU renderer**: Uses Skia for rasterization, reads back RGBA pixels. FFmpeg encoding lives in `lumen-server`.

## Guardrails

- No `todo!()`, `panic!()`, `unwrap()`, or `expect()` in runtime paths. Use `Result<T, E>` with typed errors.
- Keep the GPU renderer context long-lived — don't recreate GPU contexts per frame.
- Each `SkiaRenderer` instance is single-threaded. The orchestrator creates one per worker.
- Frame/decode caches must be bounded. Add backpressure before adding capacity.
- Local media paths must resolve against allowlisted roots only. Reject `..`, symlinks outside root, and non-file URI schemes.
- Timeline duration is capped at 1,000,000 frames to prevent OOM from malformed input.
- Run `cargo check -p lumen` and `cargo test -p lumen` after changes.

## Related Crates

- `crates/lumen-server` — Render executor, Runpod adapter binary.
- `packages/canvas-renderer` — CanvasKit-based browser preview renderer.
- `crates/lumen-local` — CLI for local rendering (`--project <path> --output <path>`).

## Commands

```bash
cargo check -p lumen
cargo test -p lumen
cargo clippy -p lumen
```
