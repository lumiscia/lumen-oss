# Implementation Plan: Lumen WASM + Node Editor Rework

**Branch**: `003-lumen-wasm-node-editor` | **Date**: 2026-02-22 | **Spec**: [spec.md](./spec.md)
**Input**: Feature specification from `/specs/003-lumen-wasm-node-editor/spec.md`

## Summary

Rework the `@lumiscia/lumen` TypeScript package and `apps/editor` to support the new lumen compositing engine architecture. The lumen-wasm crate provides a browser-accessible renderer via emscripten C ABI. The JS layer manages media decoding (mediabunny/WebCodecs) and feeds decoded frames to the WASM renderer. The editor app is rebuilt as a React Flow node editor that constructs node graphs matching the lumen engine's composition model, serializing to/from the JSON delegate format. Deprecated packages (`lumen-jsx`, `editor`, `templates`) are removed.

## Technical Context

**Language/Version**: TypeScript 5.9+, Rust 2024 edition
**Primary Dependencies**: React 19, @xyflow/react (React Flow v12+), mediabunny 1.31+, zustand 5, Vite 7, Tailwind v4
**Storage**: N/A (browser-only, in-memory state; compositions exported as JSON files)
**Testing**: Vitest (unit/integration), Playwright (e2e)
**Target Platform**: Browser (WASM + JS; WebCodecs support: Chrome 94+, Edge 94+, Safari 16.4+)
**Project Type**: Web application (SPA editor) + library (`@lumiscia/lumen` canvas/wasm integration)
**Performance Goals**: <200ms single frame render at 1080p with 20 nodes; 15fps video preview playback
**Constraints**: Bounded LRU caches for media (<512 video frames, <256 images), off-main-thread WASM rendering via Web Worker, single-threaded WASM (no SharedArrayBuffer)
**Scale/Scope**: 16 node types, up to hundreds of nodes per composition, single user per browser session

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

- [x] All external inputs and trust boundaries are identified, with explicit validation strategy.
  - JSON delegate import: validated against schema (SR-001)
  - Media source URLs: validated before fetch (SR-002)
  - WASM module binary: load errors caught and reported (OR-001)
  - Expression strings: treated as data, evaluated only in WASM sandbox (SR-003)
- [x] Contract and schema changes are mapped to all impacted consumers.
  - JSON delegate TS types mirror Rust `JsonComposition` schema exactly
  - Removing `lumen-jsx`/`editor`/`templates` packages — all consumers identified in research.md R5
  - `packages/lumen/src/index.ts` capability manifest updated to remove stale requires
- [x] Security impact is reviewed (auth, secrets, data access, abuse/failure modes).
  - No secrets involved (browser-only, no auth in editor)
  - Expression strings never evaluated as JS (SR-003)
  - Media URLs are user-provided, validated, fetched via standard fetch() (SR-002)
  - Bounded caches prevent memory exhaustion (SR-004)
- [x] Tests cover the changed behavior at the correct boundary level.
  - JSON delegate round-trip tests (vitest): serialize → parse → re-serialize identity
  - WASM integration tests: load composition → render frame → verify pixels
  - Graph validation tests: cycle detection, port type mismatch, missing required connections
  - Editor component tests: node creation, connection, property editing
- [x] Operational safeguards are defined (bounded queues/caches, observability, rollback path).
  - LRU caches with configurable limits (FR-007)
  - Render timeout with frame skip (OR-002)
  - WASM resource disposal on unmount (OR-003)
  - Structured error messages for all failure categories (OR-001)
  - Trace collection for render performance monitoring (existing pattern)

## Project Structure

### Documentation (this feature)

```text
specs/003-lumen-wasm-node-editor/
├── plan.md              # This file
├── spec.md              # Feature specification
├── research.md          # Phase 0 research findings
├── data-model.md        # Entity model and relationships
├── quickstart.md        # Developer onboarding guide
├── contracts/
│   └── json-delegate-types.md  # JSON delegate TypeScript type contract
├── checklists/
│   └── requirements.md  # Specification quality checklist
└── tasks.md             # Phase 2 output (created by /speckit.tasks)
```

### Source Code (repository root)

```text
packages/lumen/
├── src/
│   ├── canvas/                  # EXISTING: WASM bindings, preview renderer, media store
│   │   ├── wasm.ts              # LumenWasmBindings (low-level emscripten interface)
│   │   ├── renderer.ts          # LumenPreviewRenderer (high-level render lifecycle)
│   │   ├── media.ts             # MediaStore + VideoFrameDecoder (mediabunny)
│   │   ├── surface.ts           # RenderSurface (canvas pixel presentation)
│   │   ├── types.ts             # Shared types
│   │   └── index.ts             # Canvas module exports
│   ├── json-delegate/           # NEW: JSON delegate TS types and serialization
│   │   ├── types.ts             # JsonComposition, JsonNode, JsonConnection, etc.
│   │   ├── schema.ts            # Default values, schema revision constant
│   │   ├── serialize.ts         # EditorGraph → JsonComposition
│   │   ├── deserialize.ts       # JsonComposition → EditorGraph
│   │   ├── validate.ts          # Graph validation (cycles, ports, required nodes)
│   │   └── index.ts             # Module exports
│   └── index.ts                 # Package root exports (updated)
├── tests/
│   ├── json-delegate.test.ts    # Round-trip serialization tests
│   └── validate.test.ts         # Graph validation tests
└── package.json                 # Updated (no breaking dep changes)

apps/editor/
├── src/
│   ├── main.tsx                 # REWRITTEN: App entry point
│   ├── App.tsx                  # REWRITTEN: Root component with ReactFlowProvider
│   ├── store/                   # NEW: Zustand stores
│   │   ├── composition.ts       # Graph state (nodes, edges, composition metadata)
│   │   ├── timeline.ts          # Playback state (current frame, playing/paused, fps)
│   │   └── preview.ts           # Preview renderer state (worker, loading, stats)
│   ├── nodes/                   # NEW: React Flow custom node components
│   │   ├── registry.ts          # NodeTypeDef registry + default properties
│   │   ├── base-node.tsx        # Shared node wrapper (handles, selection, label)
│   │   ├── source-nodes.tsx     # Shape, MediaIn, SolidColor, Text
│   │   ├── process-nodes.tsx    # Transform, Crop, Resize, Blur, Shadow, ShapeRenderer
│   │   ├── composite-nodes.tsx  # Merge, Boolean, Switch
│   │   └── terminal-nodes.tsx   # MediaOutput, Memo, FrameHold
│   ├── components/              # NEW: UI components
│   │   ├── node-palette.tsx     # Sidebar with draggable node types
│   │   ├── inspector/           # Property editor panel
│   │   │   ├── inspector.tsx    # Inspector container (shows selected node props)
│   │   │   ├── property-fields.tsx # Per-type input controls (color, number, enum, etc.)
│   │   │   └── keyframe-editor.tsx # Keyframe track add/edit/remove UI
│   │   ├── preview-panel.tsx    # Canvas preview + play/pause + stats overlay
│   │   ├── timeline-bar.tsx     # Frame scrubber / playhead
│   │   └── toolbar.tsx          # Import/export buttons, render settings
│   ├── preview/                 # NEW: Preview worker integration
│   │   ├── worker.ts            # Web Worker (loads WASM, renders frames)
│   │   └── use-preview.ts       # React hook connecting worker to component
│   ├── lib/                     # NEW: Utilities
│   │   ├── graph-utils.ts       # Cycle detection, topological sort, port matching
│   │   ├── id-generator.ts      # Monotonic u64 ID generation
│   │   └── serialization.ts     # Import/export JSON delegate (file I/O)
│   └── styles/                  # Tailwind styles
│       └── globals.css
├── tests/
│   ├── graph-utils.test.ts      # Cycle detection, validation
│   ├── serialization.test.ts    # JSON round-trip
│   └── nodes.test.ts            # Node registry, default properties
└── package.json                 # Updated deps (@xyflow/react replaces @lumiscia/editor)

# REMOVED packages:
packages/lumen-jsx/              # DELETED
packages/editor/                 # DELETED
packages/templates/              # DELETED (depends on lumen-jsx)
```

**Structure Decision**: This follows the existing monorepo pattern. `packages/lumen` is the shared library (WASM bindings, JSON delegate types, media store). `apps/editor` is the consumer application. No new packages are created. The `json-delegate` module is added within `packages/lumen` because the types are shared between the editor and any future consumer of the JSON delegate format.

## Complexity Tracking

| Violation | Why Needed | Simpler Alternative Rejected Because |
|-----------|------------|-------------------------------------|
| Removing 3 packages (lumen-jsx, editor, templates) | Old architecture incompatible with new node-graph model | Keeping them adds confusion and false dependency signals; they would require extensive rework |
| Web Worker for rendering | Required for non-blocking UI during WASM frame renders | Main-thread rendering blocks the editor UI, violating SC-006 |
