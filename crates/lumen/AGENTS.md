# Lumen Codebase Index

## Snapshot
- Language: Rust (edition `2024`)
- Workspace crates:
  - `crates/lumen`
  - `crates/lumen-server`
  - `crates/lumen-wasm`
  - `crates/lumen-example`
- Server API status: canonical path is `POST /renders` with async job lifecycle endpoints.

## Purpose
Lumen is a GPU-first video rendering workspace:
- `lumen`: project compiler + source-pipeline mapping + Vello GPU frame renderer.
- `lumen-server`: authenticated render API + bounded background workers + FFmpeg decode/encode backend.
- `lumen-wasm`: backend-agnostic timeline/runtime bridge for web clients (future WebCodecs target).
- `lumen-example`: end-to-end short-form render example.

## Repository Layout
- `Cargo.toml`: workspace manifest.
- `crates/lumen`: core project model, compile pipeline, and GPU rendering.
- `crates/lumen-server`: API, middleware, object/job stores, worker pipeline, FFmpeg backend.
- `crates/lumen-wasm`: wasm runtime bindings and frame planning/decode request bridge.
- `crates/lumen-example`: local render demo writing MP4 output.
- `docs/vello-gpu-architecture.md`: architecture notes from Vello-based redesign.
- `Dockerfile`: container build/runtime image definition.
- `cloudbuild.yaml`: build/push pipeline.
- `service.yaml`: Cloud Run/Knative deployment spec.

## Active Runtime Flow
1. Client sends `POST /renders` with a `Project` payload (`sources`, `layers`, `timeline.total_frames`).
2. API compiles/validates the project, stores payload, and enqueues job.
3. Worker compiles to `CompiledTimeline`, prepares media decode requirements, and renders frames with Vello.
4. FFmpeg backend encodes MP4 and stores artifact in object store.
5. Clients poll `GET /renders/{job_id}` for status.
6. Clients fetch output from `GET /renders/{job_id}/artifact`.
7. Clients fetch previews from `GET /renders/{job_id}/frames/{frame_index}`.

## Dev/Build Commands
- Check workspace: `cargo check`
- Compile tests: `cargo test --no-run`
- Run tests: `cargo test`
- Run server: `SECRET=<token> cargo run -p lumen-server`
- Run short-form example: `cargo run -p lumen-example`

## Strict Session Guardrails
These are required for future sessions unless the user explicitly requests otherwise.

1. Runtime safety
- Do not introduce `todo!()`, `panic!()`, `unwrap()`, or `expect()` in runtime-critical non-test paths.
- Use typed errors with contextual messages.

2. API behavior
- Keep `/renders` endpoints as the canonical public server surface unless explicitly asked to change API shape.
- Return sanitized internal errors to clients; keep detailed internals in logs.

3. Auth
- Bearer checks must include explicit malformed-header handling and constant-time token comparison.
- Add/maintain auth tests whenever middleware behavior changes.

4. Async and blocking work
- CPU-heavy or blocking file/media operations must run off the async reactor (`spawn_blocking` or dedicated workers).
- Avoid expensive frame rendering work directly on request executor threads.

5. Storage and memory
- Keep in-memory queue/job/object stores bounded.
- Preserve cleanup/TTL behavior for terminal jobs and artifacts.
- Avoid unbounded decoded-frame caches; prefer bounded queues/caches with backpressure.

6. Media input safety
- Local media path resolution must stay constrained to an allowlisted/canonical root.
- Reject traversal and unsupported URI schemes.

7. Rendering correctness
- Clip opacity must behave consistently across text, shape, image, and video rendering paths.
- Source pipeline mapping (`trim`, `speed`, `reverse`, `looping`) should remain deterministic and tested.
- Any rendering behavior change should include a regression test.

8. Deployment correctness
- Container runtime must match produced binary name and libc expectations.
- `service.yaml` secret refs must include required key fields.

9. Review hygiene
- Remove or gate dead experimental modules that generate warnings and are not wired into runtime flow.
- Keep `cargo check` warning count at zero for actively compiled paths where feasible.

10. Validation before handoff
- Always run at least `cargo check` and `cargo test --no-run` after substantive changes.
- If tests are added or behavior changes in endpoints/middleware/rendering, run targeted `cargo test`.

## Commit Style (Owner)
Use this style when committing on behalf of the owner:
- Format: `<type>: <short lowercase summary>`
- Preferred types: `feat`, `fix`, `chore`
- Keep subject concise and pragmatic; no trailing period.
- Use one focused commit per request unless asked to split.

Examples from repo history:
- `fix: stabilize video frame decode to prevent flashing`
- `feat: add async render worker and lifecycle endpoints`
- `chore: remove .DS_Store artifacts`

## Current Known Focus Areas
- Expand negative-path tests for project compile validation and source pipeline rules.
- Maintain bounded worker/decode/object memory behavior under load.
- Improve decode/render/encode throughput while keeping deterministic output.
