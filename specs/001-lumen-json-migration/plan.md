# Implementation Plan: Lumen JSON Format Migration

**Branch**: `001-lumen-json-migration` | **Date**: 2026-02-22 | **Spec**: [/specs/001-lumen-json-migration/spec.md](./spec.md)
**Input**: Feature specification from `/specs/001-lumen-json-migration/spec.md`

## Summary

Migrate editor and authoring workflows to one canonical project JSON contract aligned with the renderer JSON feature path, complete full cutover away from preview-era versioned project handling, and expand the JSX rewrite contract to explicitly cover required primitives, component composition rules, deterministic compilation, and actionable compiler diagnostics.

## Technical Context

**Language/Version**: TypeScript (workspace packages and app), Rust (consumer contract under `crates/lumen` JSON feature)  
**Primary Dependencies**: `packages/lumen` compile pipeline, `packages/lumen-jsx` JSX runtime/compiler layer, `packages/templates` preset builders, editor preview/export path, schema/contract fixtures, Vitest  
**Storage**: N/A (contract and fixture files in repository)  
**Testing**: Vitest contract/parity tests in affected packages, targeted Rust JSON feature tests for consumer compatibility  
**Target Platform**: Browser-based editor workflow plus Node-compatible package compilation pipelines  
**Project Type**: Monorepo web app + shared libraries + compiler-style transform layer  
**Performance Goals**: Maintain deterministic compile output for equivalent JSX input and keep editor/template export p95 latency regression within 5% of current baseline fixture runs  
**Constraints**: Full cutover in touched areas (no legacy versioned output path), fail-closed validation for invalid/mixed payloads, canonical JSON contract parity across all four workflows, and explicit authorization enforcement on renderer-bound serialization/submission entrypoints  
**Scale/Scope**: 4 in-scope workflows (`apps/editor`, `packages/lumen`, `packages/lumen-jsx`, `packages/templates`) plus compatibility checks against `crates/lumen` JSON feature consumer

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

- [x] All external inputs and trust boundaries are identified, with explicit validation strategy.
  - Editor export input, template input, and JSX input are explicit boundaries; plan requires schema validation and fail-closed behavior before renderer submission.
- [x] Contract and schema changes are mapped to all impacted consumers.
  - Producer workflows (editor, lumen, lumen-jsx, templates) and consumer contract (`crates/lumen` JSON feature) are included in scope.
- [x] Security impact is reviewed (auth, secrets, data access, abuse/failure modes).
  - Security scope includes malformed/mixed payload rejection, sanitized diagnostics, and authorization checks for editor export invocation, template build invocation, and JSX compile-to-render invocation.
- [x] Tests cover the changed behavior at the correct boundary level.
  - Contract/parity tests, authorization rejection tests, and invalid-input diagnostics tests are required at compile/export boundaries.
- [x] Operational safeguards are defined (bounded queues/caches, observability, rollback path).
  - This feature does not introduce new async workers/queues; safeguard is strict contract gating and deterministic outputs to reduce rollout risk.


**Post-Design Re-check (after Phase 1 artifacts)**: PASS
- Research decisions preserve boundary validation and fail-closed behavior.
- Data model, contracts, and quickstart enforce full cutover and cross-consumer contract alignment.
- No new constitutional violations introduced by design outputs.
## Project Structure

### Documentation (this feature)

```text
specs/001-lumen-json-migration/
├── plan.md
├── research.md
├── data-model.md
├── quickstart.md
├── contracts/
└── tasks.md
```

### Source Code (repository root)

```text
apps/editor/
├── src/preview/
│   ├── project-schema.ts
│   ├── use-preview.ts
│   └── use-preview-helpers.ts
└── src/lib/lumen-client.ts

packages/lumen/
├── src/compile.ts
├── src/pipeline.ts
├── src/contracts/renderer-contract.ts
└── src/**/*.test.ts

packages/lumen-jsx/
├── src/jsx.ts
├── src/components/*.tsx
└── src/**/*.test.ts

packages/templates/
└── src/presets/**

crates/lumen/
└── src/json/**
```

**Structure Decision**: Use the existing monorepo multi-package structure. Planning and implementation stay within the four producer workflows, with contract verification against the Rust consumer JSON feature path.

## Complexity Tracking

No constitutional violations requiring justification.