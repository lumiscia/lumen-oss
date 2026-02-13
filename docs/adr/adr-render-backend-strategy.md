# ADR: Render Backend Strategy (Rust Orchestration + Vello/Skia)

- Status: Amended
- Date: 2026-02-13
- Amended: 2026-02-12
- Owners: Rendering platform team

## Context

Lumiscia currently renders on the server with Rust orchestration and a Vello GPU renderer.
We need to evaluate a C++ Skia GPU renderer while preserving production safety, reproducibility,
and a deterministic migration path.

## Decision

1. Rust remains the orchestration layer for job control, timeline compilation, source pipeline
   mapping, decode/render/encode coordination, and FFmpeg process management.
2. A C++ Skia GPU backend is introduced behind an incremental backend boundary.
3. Vello stays as an existing backend for comparison, fallback, and production rollback.
4. Skia Graphite is the first production GPU target (Ganesh skipped; Graphite is production-ready
   as of Skia m138+).
5. Feature flags (`renderer-vello` / `renderer-skia`) in `lumen-server` control backend selection.

## Platform GPU Targets

- Linux production target order:
  - Primary: Skia Graphite on Vulkan
  - Fallback: Vello backend
- macOS production target order:
  - Primary: Skia Graphite on Metal
  - Fallback: Vello backend

If primary initialization fails or runtime error-rate thresholds are exceeded, rendering must
fallback to the configured Vello path without changing timeline semantics.

## Web Preview Backend

A TypeScript CanvasKit renderer (`@lumiscia/canvas-renderer`) provides feature-equivalent browser
preview rendering. It consumes the same `DrawOperation` structures and applies identical
layout/fit/compositing logic, with known approximations:

- Text rendering is marked "approximate" (browser font stack differs from server Roboto).
- Dropped video frames fall back to last-good-frame with an "approximate" badge.

The CanvasKit renderer replaces the previous `lumen-wasm` WASM runtime approach.

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
5. Do not remove Vello until Skia meets correctness and reliability gates for burn-in.

## Rollout Policy

1. Server rendering remains the source of truth for final output.
2. Production rollout must use backend feature flags and canary cohorts.
3. Rollback thresholds (error-rate, timeout-rate, throughput degradation) must be defined before
   canary starts.
4. Vello remains runnable during the full Skia burn-in period.

## Consequences

- Positive:
  - controlled migration risk,
  - reproducible backend comparison,
  - explicit fallback path.
- Tradeoff:
  - temporary dual-backend maintenance burden.

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
