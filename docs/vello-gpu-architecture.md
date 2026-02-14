# Lumen V0 GPU Architecture (Historical Vello + FFmpeg)

## Deprecation Status

Vello has been removed from the Lumiscia runtime codebase. Skia is the preferred renderer and is
better across our production metrics (visual correctness, throughput, and operational reliability).
This document is kept for historical context only.
New compositing features (clip groups and alpha masks) were never implemented on the Vello path.

## Inputs From Vello Source
This design follows the render path used in Vello's own code and examples:

- `examples/headless/src/main.rs`: headless render-to-texture and explicit GPU readback.
- `vello::util::RenderContext`: reusable `wgpu::Instance` + device lifecycle.
- `Renderer::render_to_texture`: compute-driven scene rasterization to `Rgba8Unorm` textures.

Key carry-over:
- Create one long-lived GPU renderer context per worker.
- Render every frame into a storage texture, then copy/read for encode.
- Keep render submission serialized on one thread to avoid `wgpu` context churn.

## Core Data Model (new `lumen` crate)

### Project
A project is now source/layer driven (ffmpeg/remotion style):

- `sources`: reusable media or generator nodes.
- `layers`: timeline clips referencing sources or procedural graphics.
- `timeline`: fixed `fps` + `total_frames`.

### Source Pipeline
Video clips reference source pipelines, not raw asset IDs:

- `trim(start_frame, end_frame)`
- `speed(factor)`
- `reverse`
- `loop(count | infinite)`

This keeps clip-level logic declarative and similar to ffmpeg filter chains.

### Compilation
Compilation outputs a frame schedule:

- `CompiledTimeline { frame_ops[frame_index] }`
- each op includes resolved z-order, transform, opacity, and source-frame mapping.

No API compatibility with the old track schema is preserved.

## Runtime Pipeline (`lumen-server`)

### Stage 1: Compile + Validate
- Parse request into `Project`.
- Compile to `CompiledTimeline`.
- Validate source references and frame bounds.

### Stage 2: Decode (parallel workers)
- Decode source clips with FFmpeg backend.
- Prefer hardware decode: `-hwaccel auto`.
- Decode is bounded by per-source frame cache capacity to avoid unbounded memory.

### Stage 3: GPU Render (single submit thread)
- One thread owns `vello::Renderer` + `wgpu` device/queue.
- For each frame:
  - gather active ops
  - build `vello::Scene`
  - render to texture
  - read RGBA for encode stage

### Stage 4: Encode (separate thread/process)
- Feed raw RGBA frames into FFmpeg encoder process.
- Prefer hardware encoders when available (`h264_videotoolbox`, `h264_nvenc`, `h264_qsv`), fallback to `libx264`.
- Keep encode I/O streaming and bounded.

## Concurrency Rules
- GPU submit stays single-threaded.
- Decode/IO/encode run in parallel and communicate through bounded channels.
- No frame fan-out cloning of large buffers outside bounded queues.

## Crates
- `lumen-server`: FFmpeg backend + job orchestration.
- `@lumiscia/canvas-renderer`: CanvasKit preview renderer aligned with Skia semantics.

## Performance Principles
- Reuse GPU resources per job.
- Keep source decode sequential when possible for cache locality.
- Avoid per-request renderer init.
- Keep queues bounded and backpressured.
