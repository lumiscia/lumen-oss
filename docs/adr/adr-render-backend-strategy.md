# ADR: Render Backend Strategy (Rust Orchestration + Skia)

- Status: Amended
- Date: 2026-02-13
- Amended: 2026-02-12
- Owners: Rendering platform team

## Context

Lumiscia renders on the server with Rust orchestration and a Skia backend.
Skia has outperformed Vello for Lumiscia workloads in correctness, throughput, and operational
reliability, and the Vello backend has now been removed.

## Decision

1. Rust remains the orchestration layer for job control, timeline compilation, source pipeline
   mapping, decode/render/encode coordination, and FFmpeg process management.
2. A C++ Skia GPU backend is introduced behind an incremental backend boundary.
3. Vello is removed from the runtime codebase.
4. Skia Graphite is the first production GPU target (Ganesh skipped; Graphite is production-ready
   as of Skia m138+).
5. `renderer-skia` is the only renderer backend feature in `lumen-server`.
6. New compositing features (clip groups and alpha masks) ship only on Skia + CanvasKit preview.

## Platform GPU Targets

- Linux production target order:
  - Primary: Skia Graphite on Vulkan
- macOS production target order:
  - Primary: Skia Graphite on Metal

Skia is the only production path.

## Web Preview Backend

A WASM-backed renderer (`@lumiscia/canvas-renderer` + `lumen-wasm`) provides feature-equivalent
browser preview rendering. It runs the Rust Skia backend compiled for `wasm32-unknown-emscripten`,
and uses Mediabunny (WebCodecs) for client-side media decoding, with known approximations:

- Text rendering is marked "approximate" (browser font stack differs from server Roboto).
- Dropped video frames fall back to last-good-frame with an "approximate" badge.

The WASM renderer replaces the previous CanvasKit preview approach.

## Success Criteria

The rollout gate requires all of the following on the benchmark harness and fixture set:

1. Latency:
   - p95 render stage frame latency <= 35 ms at 1080p for interactive profile fixtures.
2. Throughput:
   - end-to-end throughput >= 30 fps equivalent on at least one benchmark machine class.
3. Correctness:
   - golden-frame pass rate >= 99.9% against server reference fixtures,
   - no critical visual regressions in transform, opacity, clip ordering, and text placement.

Numbers are contract targets and can only be changed by a new ADR update.

## Migration Guardrails

1. No full renderer rewrite in one step.
2. Introduce and enforce a backend boundary in Rust first.
3. Keep FFmpeg process model unchanged through benchmark phases to isolate renderer effects.
4. Do not compare backends with different scenes, codecs, hardware classes, or harness logic.
5. Keep historical Vello notes in docs only; no runtime Vello support remains.

## Rollout Policy

1. Server rendering remains the source of truth for final output.
2. Production rollout uses Skia feature flags and canary cohorts.
3. Rollback thresholds (error-rate, timeout-rate, throughput degradation) must be defined before
   canary starts.
4. Vello is not part of the runtime rollout path.

## Consequences

- Positive:
  - controlled migration risk,
  - single-renderer operational focus.
- Tradeoff:
  - no renderer fallback backend in runtime.

## Execution Contract / Exit Criteria

This ADR is complete when:

1. It is approved by rendering owners.
2. Subsequent implementation phases reference this ADR.
3. Any deviation is documented via ADR amendment before code changes.

## Amendment Log

### 2026-02-12: Graphite-first, CanvasKit web preview

- Point 4 changed: "Skia Graphite is the first production GPU target" (Ganesh skipped).
- Point 5 removed: Graphite is no longer deferred.
- Platform targets updated: Graphite on Vulkan (Linux) / Metal (macOS).
- New section: "Web Preview Backend" documenting CanvasKit + MediaProvider role.
- `lumen-wasm` removed from workspace; replaced by `@lumiscia/canvas-renderer`.

### 2026-02-14: Skia-first default, Vello deprecation

- Decision point 3 amended: Vello is explicitly deprecated.
- Skia is the preferred renderer across quality, performance, and reliability for Lumiscia.
- New mask/group compositing work is Skia + CanvasKit only; Vello receives no new feature support.

### 2026-02-14: Vello runtime removal

- Decision point 3 amended again: Vello backend removed from runtime code.
- `renderer-vello` feature removed from `lumen` and `lumen-server`.
- Skia is now the only renderer backend path.

### 2026-02-19: WASM preview renderer reintroduced

- Web preview now runs the Rust Skia backend via `lumen-wasm` (emscripten).
- `@lumiscia/canvas-renderer` wraps the WASM module and Mediabunny (WebCodecs) media decode.
