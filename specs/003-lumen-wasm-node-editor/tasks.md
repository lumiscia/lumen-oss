# Tasks: Lumen WASM + Node Editor Rework

**Input**: Design documents from `/specs/003-lumen-wasm-node-editor/`
**Prerequisites**: plan.md (required), spec.md (required), research.md, data-model.md, contracts/

**Tests**: Include test tasks for every behavior-changing story. If a story is documentation-only or scaffolding-only, explicitly state why no new automated test is required.

**Organization**: Tasks are grouped by user story to enable independent implementation and testing of each story.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (e.g., US1, US2, US3)
- Include exact file paths in descriptions

## Path Conventions

- **Library**: `packages/lumen/src/`
- **Editor app**: `apps/editor/src/`
- **Tests (lib)**: `packages/lumen/tests/`
- **Tests (editor)**: `apps/editor/tests/`

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Project initialization and dependency installation

- [x] T001 Install `@xyflow/react` and `zustand` in `apps/editor/` (`pnpm --filter @lumiscia/editor-app add @xyflow/react zustand`)
- [x] T002 [P] Create `packages/lumen/src/json-delegate/` directory with `index.ts` barrel export
- [x] T003 [P] Create `apps/editor/src/store/`, `apps/editor/src/nodes/`, `apps/editor/src/components/`, `apps/editor/src/preview/`, `apps/editor/src/lib/` directory structure

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Core types and infrastructure that MUST be complete before ANY user story can be implemented

**⚠️ CRITICAL**: No user story work can begin until this phase is complete

- [x] T004 [US-ALL] Define JSON delegate TypeScript types in `packages/lumen/src/json-delegate/types.ts` — all interfaces/types from `contracts/json-delegate-types.md`: `JsonComposition`, `JsonGraph`, `JsonNode`, `JsonNodeKind` (16 variants), `JsonConnection`, `JsonPort`, all supporting enums, `JsonKeyframeTrack`, `JsonKeyframe`, `JsonExpression`, `JsonTimelineSettings`, `JsonRenderSettings`
- [x] T005 [US-ALL] Define schema constants and default values in `packages/lumen/src/json-delegate/schema.ts` — `SCHEMA_REVISION` constant, `createDefaultComposition()`, `createDefaultRenderSettings()`, `createDefaultTimelineSettings()
- [x] T006 [P] [US-ALL] Define `NodeTypeDef` registry in `apps/editor/src/nodes/registry.ts` — all 16 node type definitions with labels, categories, input/output port definitions (from research.md R3), and default property values
- [x] T007 [P] [US-ALL] Define editor graph types in `apps/editor/src/lib/graph-utils.ts` — `EditorNode` (extends React Flow `Node` with `JsonNodeKind` data + port info), `EditorEdge` (extends React Flow `Edge` with port metadata), cycle detection via `getOutgoers`, port type compatibility check, `isValidConnection` callback
- [x] T008 [US-ALL] Create `packages/lumen/src/json-delegate/index.ts` barrel export and add `json-delegate` to `packages/lumen/src/index.ts` package exports

**Checkpoint**: Foundation ready — JSON delegate types, node registry, and graph utilities are available. User story implementation can now begin.

---

## Phase 3: User Story 1 — Load and Render a Composition via WASM (Priority: P1) 🎯 MVP

**Goal**: Adapt the existing WASM bindings to accept `JsonComposition` and render frames

**Independent Test**: Construct a minimal `SolidColor → MediaOutput` composition in JSON delegate format, load into WASM, render frame 0, verify RGBA pixel output

### Tests for User Story 1 (REQUIRED for behavior changes) ⚠️

> **NOTE: Write these tests FIRST, ensure they FAIL before implementation**

- [x] T009 [P] [US1] Unit test in `packages/lumen/tests/json-delegate.test.ts` — test `createDefaultComposition()` produces valid structure, test JSON.stringify/parse round-trip preserves all fields
- [x] T010 [P] [US1] Integration test concept in `packages/lumen/tests/wasm-render.test.ts` — test that `LumenPreviewRenderer.loadComposition(solidColorComposition)` + `renderFrame(0)` succeeds (requires WASM binary; mark as integration test)

### Implementation for User Story 1

- [x] T011 [US1] Adapt `packages/lumen/src/canvas/renderer.ts` — modify `LumenPreviewRenderer` to accept `JsonComposition` (from `json-delegate/types.ts`) instead of old `Project` type. Update `loadProject` → `loadComposition` to serialize `JsonComposition` to JSON string and pass to `lumen_wasm_load_project`. Update frame render loop to use JSON delegate timeline settings.
- [x] T012 [US1] Adapt `packages/lumen/src/canvas/wasm.ts` — update `LumenWasmBindings` type annotations to document that `lumen_wasm_load_project` accepts JSON delegate string. Update `getFrameRequirements` return type to use source IDs from JSON delegate format (media_in node source fields).
- [x] T013 [US1] Update `packages/lumen/src/canvas/types.ts` — add `CompositionLoadResult`, `FrameRenderResult`, `RenderStats` types aligned with JSON delegate format. Remove/replace old `Project`-based types if present.

**Checkpoint**: WASM renderer accepts JSON delegate compositions and renders frames. Library layer is functional.

---

## Phase 4: User Story 2 — Decode and Supply Video Frames via JS Media Store (Priority: P1)

**Goal**: Adapt existing media store to work with JSON delegate media source references

**Independent Test**: Create composition with `MediaIn(Video)` node, provide test video, render frame, verify decoded video frame is composited

### Tests for User Story 2 (REQUIRED for behavior changes) ⚠️

- [x] T014 [P] [US2] Unit test in `packages/lumen/tests/media-store.test.ts` — test `MediaStore` LRU cache eviction at capacity limit, test `extractMediaSources(composition)` correctly finds all `media_in` nodes and returns source IDs

### Implementation for User Story 2

- [x] T015 [US2] Add `extractMediaSources()` utility in `packages/lumen/src/canvas/media.ts` — given a `JsonComposition`, walk the graph and return all unique media source IDs (image sources + video sources with their ranges/speeds/loop modes)
- [x] T016 [US2] Adapt media store coordination in `packages/lumen/src/canvas/renderer.ts` — update the render loop (query frame requirements → decode → upload → render) to derive media source metadata from `JsonComposition.graph.nodes` filtered to `media_in` kind, replacing old layers/clips format
- [x] T017 [US2] Verify `packages/lumen/src/canvas/media.ts` — confirm `MediaStore` and `VideoFrameDecoder` classes work with source ID strings from JSON delegate format. Minimal changes expected (existing classes are source-ID-based). Ensure LRU cache limits are configurable via constructor.

**Checkpoint**: Full render pipeline works: JSON delegate → WASM renderer → media store supplies decoded video/image frames → rendered output

---

## Phase 5: User Story 3 — Build a Node Graph in the Editor (Priority: P1) 🎯 MVP

**Goal**: React Flow node editor with all 16 node types, port connections, cycle prevention, inspector

**Independent Test**: Add SolidColor + MediaOutput nodes, connect them, verify graph is valid and serializable

### Tests for User Story 3 (REQUIRED for behavior changes) ⚠️

> **NOTE: Write these tests FIRST, ensure they FAIL before implementation**

- [x] T018 [P] [US3] Unit test in `apps/editor/tests/graph-utils.test.ts` — test cycle detection (reject A→B→A), test port type compatibility (reject RasterFrame→Vector), test single-connection-per-input enforcement, test self-loop rejection
- [x] T019 [P] [US3] Unit test in `apps/editor/tests/nodes.test.ts` — test all 16 node type definitions in registry have correct port counts/types, test `createDefaultComposition()` includes MediaOutput node

### Implementation for User Story 3

- [x] T020 [US3] Create composition Zustand store in `apps/editor/src/store/composition.ts` — nodes state (React Flow `Node[]`), edges state (React Flow `Edge[]`), composition metadata (render settings, timeline settings), actions: `addNode`, `removeNode`, `addEdge`, `removeEdge`, `updateNodeProperties`, `setRenderSettings`, `setTimelineSettings`. MediaOutput node created by default and protected from deletion.
- [x] T021 [US3] Create base node component in `apps/editor/src/nodes/base-node.tsx` — shared wrapper rendering `<Handle>` components for each port from `NodeTypeDef`, node label, selection highlight. Handles use port `kind` as data attribute for connection validation.
- [x] T022 [P] [US3] Create source node components in `apps/editor/src/nodes/source-nodes.tsx` — `ShapeNode`, `MediaInNode`, `SolidColorNode`, `TextNode`. Each uses `base-node.tsx` wrapper, shows compact property preview in node body.
- [x] T023 [P] [US3] Create processing node components in `apps/editor/src/nodes/process-nodes.tsx` — `ShapeRendererNode`, `TransformNode`, `CropNode`, `ResizeNode`, `BlurNode`, `ShadowNode`, `FrameHoldNode`, `MemoNode`. Each uses `base-node.tsx` wrapper.
- [x] T024 [P] [US3] Create compositing node components in `apps/editor/src/nodes/composite-nodes.tsx` — `MergeNode`, `BooleanNode`, `SwitchNode`. Each uses `base-node.tsx` wrapper.
- [x] T025 [US3] Create terminal node component in `apps/editor/src/nodes/terminal-nodes.tsx` — `MediaOutputNode`. Uses `base-node.tsx` wrapper. Visually distinct (different color/border) to indicate it's required.
- [x] T026 [US3] Create node palette sidebar in `apps/editor/src/components/node-palette.tsx` — lists all 16 node types grouped by category (source, processing, compositing, terminal). Supports drag-to-add onto canvas. Uses `NodeTypeDef` registry.
- [x] T027 [US3] Create inspector panel in `apps/editor/src/components/inspector/inspector.tsx` — shows properties of selected node. Renders `property-fields.tsx` for each property of the node's `JsonNodeKind`.
- [x] T028 [US3] Create property field components in `apps/editor/src/components/inspector/property-fields.tsx` — per-type input controls: color picker (RGBA [number,4]), numeric input (f32/u32), boolean toggle, enum dropdown (BlendMode, ResizeMode, etc.), text input (String), range editor ({start, end}).
- [x] T029 [US3] Rewrite `apps/editor/src/App.tsx` — root component with `<ReactFlowProvider>`, layout: node palette (left), React Flow canvas (center), inspector (right), preview panel (bottom or right). Wire `isValidConnection` callback from `graph-utils.ts`. Register all node types from registry.
- [x] T030 [US3] Create stable ID generator in `apps/editor/src/lib/id-generator.ts` — monotonic u64 ID generation for nodes and keyframe tracks. Must not collide across save/load cycles.

**Checkpoint**: Node editor functional — can place all 16 node types, connect them with type checking and cycle prevention, edit properties in inspector, MediaOutput is always present.

---

## Phase 6: User Story 4 — Preview Composition in Real Time (Priority: P1) 🎯 MVP

**Goal**: Live preview rendering via WASM in Web Worker, play/pause, timeline scrubbing

**Independent Test**: Create SolidColor → MediaOutput, verify preview shows solid color, change color, verify preview updates

### Tests for User Story 4 (REQUIRED for behavior changes) ⚠️

- [ ] T031 [P] [US4] Unit test in `apps/editor/tests/serialization.test.ts` — test `serializeComposition(store)` produces valid `JsonComposition` with all required fields, test round-trip: serialize → deserialize → re-serialize produces identical JSON

### Implementation for User Story 4

- [ ] T032 [US4] Create preview Web Worker in `apps/editor/src/preview/worker.ts` — handles `WorkerInMessage` types: `init` (receive OffscreenCanvas), `loadComposition` (serialize JsonComposition, load into WASM renderer), `render` (render frame, return timing stats), `dispose`. Uses `LumenPreviewRenderer` from `@lumiscia/lumen`. Based on existing `apps/editor/src/preview-worker.ts` patterns.
- [ ] T033 [US4] Create `usePreview` hook in `apps/editor/src/preview/use-preview.ts` — manages Worker lifecycle, sends composition updates (debounced), handles play/pause/scrub, tracks render stats. Subscribes to composition store changes.
- [ ] T034 [US4] Create timeline Zustand store in `apps/editor/src/store/timeline.ts` — state: `currentFrame`, `isPlaying`, `fps` (from composition), `totalFrames` (from composition). Actions: `play`, `pause`, `setFrame`, `tick` (advance frame, loop at end).
- [ ] T035 [US4] Create preview Zustand store in `apps/editor/src/store/preview.ts` — state: `workerReady`, `isLoading`, `lastRenderTime`, `resolution`, `error`. Updated by `usePreview` hook from worker messages.
- [ ] T036 [US4] Create preview panel component in `apps/editor/src/components/preview-panel.tsx` — canvas element (receives OffscreenCanvas transfer), play/pause button, frame counter, render time display, resolution display. Uses `usePreview` hook.
- [ ] T037 [US4] Create timeline bar component in `apps/editor/src/components/timeline-bar.tsx` — frame scrubber (range input or draggable playhead), current frame / total frames display. Wired to timeline store.
- [ ] T038 [US4] Create serialization utilities in `apps/editor/src/lib/serialization.ts` — `serializeComposition(store)`: reads composition store, converts React Flow nodes/edges to `JsonComposition` format. `deserializeComposition(json)`: parses JSON, populates composition store with nodes/edges/properties. `importFromFile()` / `exportToFile()` for file I/O.
- [ ] T039 [US4] Wire preview into `apps/editor/src/App.tsx` — add preview panel and timeline bar to layout, connect composition store changes to preview worker updates (version-stamped to avoid stale renders).

**Checkpoint**: Full interactive loop: edit graph → preview updates automatically → play/pause works → timeline scrubbing shows frames → stats displayed

---

## Phase 7: User Story 5 — Edit Keyframe Animation Tracks (Priority: P2)

**Goal**: Add/edit/remove keyframe tracks on animatable node properties

**Independent Test**: Add Transform node, create keyframe track on translate_x (frame 0→0, frame 60→500, Linear), play preview, verify content moves

### Tests for User Story 5 (REQUIRED for behavior changes) ⚠️

- [ ] T040 [P] [US5] Unit test extending `apps/editor/tests/serialization.test.ts` — test keyframe track serialization: tracks array populated with correct node_id, property_path, keys, extrapolation. Test duplicate frame position rejection.

### Implementation for User Story 5

- [ ] T041 [US5] Extend composition store in `apps/editor/src/store/composition.ts` — add `tracks: JsonKeyframeTrack[]` state. Actions: `addTrack(nodeId, propertyPath, valueType)`, `removeTrack(trackId)`, `addKeyframe(trackId, frame, value, interpolation)`, `updateKeyframe(trackId, frame, updates)`, `removeKeyframe(trackId, frame)`, `setExtrapolation(trackId, before, after)`. Enforce no duplicate frame positions. Generate stable track IDs.
- [ ] T042 [US5] Create keyframe editor UI in `apps/editor/src/components/inspector/keyframe-editor.tsx` — per-property "Add Keyframe" button (appears for animatable properties), keyframe list with frame position / value / interpolation mode controls, delete keyframe button. Extrapolation dropdown (Hold/DefaultValue) per track.
- [ ] T043 [US5] Integrate keyframe editor into inspector in `apps/editor/src/components/inspector/inspector.tsx` — for each animatable property, show keyframe editor below the property field. Highlight properties that have active keyframe tracks.
- [ ] T044 [US5] Update serialization in `apps/editor/src/lib/serialization.ts` — `serializeComposition` must include `tracks` array from composition store. `deserializeComposition` must reconstruct tracks in store. Validate orphaned tracks (referencing deleted nodes) and emit warnings.

**Checkpoint**: Keyframe animation tracks can be created, edited, deleted. They serialize correctly and the preview reflects animated values during playback.

---

## Phase 8: User Story 6 — Configure All Node Types and Their Properties (Priority: P2)

**Goal**: Full property coverage for all 16 node types with appropriate input controls

**Independent Test**: Place each node type, verify all properties appear in inspector with correct types, modify each, verify serialized JSON

### Tests for User Story 6 (REQUIRED for behavior changes) ⚠️

- [ ] T045 [P] [US6] Unit test in `apps/editor/tests/nodes.test.ts` — for each of 16 node types: verify registry entry has all expected properties from data-model, verify default values produce valid `JsonNodeKind`, verify all ports match Rust definitions from research.md R3

### Implementation for User Story 6

- [ ] T046 [US6] Implement geometry sub-editor for Shape node in `apps/editor/src/components/inspector/property-fields.tsx` — geometry type selector (rectangle/ellipse/polygon), dimension fields for rect/ellipse, point list editor for polygon
- [ ] T047 [US6] Implement media kind sub-editor for MediaIn node in `apps/editor/src/components/inspector/property-fields.tsx` — media type toggle (image/video), source ID input, conditional video fields: range (start/end), speed (number), loop mode (none/repeat/ping_pong)
- [ ] T048 [US6] Implement switch map editor for Switch node in `apps/editor/src/components/inspector/property-fields.tsx` — dynamic list of entries (key string + range start/end), add/remove entry buttons
- [ ] T049 [US6] Implement text property editor for Text node in `apps/editor/src/components/inspector/property-fields.tsx` — content textarea, font family input, font size/weight numeric inputs, font style dropdown, optional max_width, color picker, alignment dropdowns (horizontal + vertical)
- [ ] T050 [US6] Verify all remaining node property editors in `apps/editor/src/components/inspector/property-fields.tsx` — ensure every property for Transform (7 fields + sampling), ShapeRenderer (5 fields), Crop (4 fields), Resize (4 fields), Blur (1), Shadow (4), Boolean (2), Merge (2), FrameHold (1), Memo (2), SolidColor (3) all have correct input controls from T028

**Checkpoint**: All 16 node types fully configurable in the inspector. No stubbed properties. Every property serializes correctly.

---

## Phase 9: User Story 7 — Export Valid JSON Delegate Format (Priority: P2)

**Goal**: Round-trip JSON delegate export/import with validation

**Independent Test**: Build composition with multiple node types, export JSON, reload into editor, verify identical graph

### Tests for User Story 7 (REQUIRED for behavior changes) ⚠️

> **NOTE: Write these tests FIRST, ensure they FAIL before implementation**

- [ ] T051 [P] [US7] Unit test in `packages/lumen/tests/validate.test.ts` — test `validateComposition`: missing MediaOutput → error, duplicate node IDs → error, connection references nonexistent node → error, cycle → error, incompatible port types → error, orphaned keyframe track → warning, valid composition → pass
- [ ] T052 [P] [US7] Unit test in `apps/editor/tests/serialization.test.ts` — full round-trip: build complex graph (10+ nodes, connections, keyframes, expressions), export JSON string, parse, reimport, assert all nodes/edges/properties/tracks/expressions identical

### Implementation for User Story 7

- [ ] T053 [US7] Implement `validateComposition()` in `packages/lumen/src/json-delegate/validate.ts` — check exactly one MediaOutput, no duplicate node IDs, all connection node/port references valid, port type compatibility, DAG check (no cycles), non-optional input ports on reachable nodes must be connected, orphaned keyframe tracks/expressions produce warnings. Return structured result with errors + warnings.
- [ ] T054 [US7] Implement `serialize()` in `packages/lumen/src/json-delegate/serialize.ts` — convert editor's graph representation to `JsonComposition`. Run `validateComposition()` before output. Throw on errors, include warnings in result.
- [ ] T055 [US7] Implement `deserialize()` in `packages/lumen/src/json-delegate/deserialize.ts` — parse JSON string, validate structure, convert to editor's graph representation (nodes with positions, edges). Handle schema version check. Return structured errors for malformed input (SR-001).
- [ ] T056 [US7] Create toolbar component in `apps/editor/src/components/toolbar.tsx` — Export JSON button (triggers serialize → download), Import JSON button (triggers file picker → deserialize → load into store), render settings controls (width, height, background color, fps, duration). Validate on import, show errors inline.
- [ ] T057 [US7] Add expression support to composition store in `apps/editor/src/store/composition.ts` — `expressions: JsonExpression[]` state. Actions: `addExpression(nodeId, propertyPath, source)`, `updateExpression(nodeId, propertyPath, source)`, `removeExpression(nodeId, propertyPath)`. Expressions are stored as data strings, not evaluated in JS.
- [ ] T058 [US7] Add expression editor to inspector in `apps/editor/src/components/inspector/inspector.tsx` — for each property, toggle between "Value" and "Expression" mode. Expression mode shows a text input for the expression source string.

**Checkpoint**: JSON delegate export produces valid documents that load in both the editor and the WASM renderer. Round-trip fidelity is verified.

---

## Phase 10: User Story 8 — Remove Deprecated Packages (Priority: P3)

**Goal**: Clean removal of `@lumiscia/lumen-jsx`, `@lumiscia/editor`, and `@lumiscia/templates`

**Independent Test**: `pnpm install` and `pnpm build` succeed after removal, no references remain

### Implementation for User Story 8

*No new tests required — this is a cleanup story. Verification is successful workspace build.*

- [ ] T059 [P] [US8] Delete `packages/lumen-jsx/` directory entirely
- [ ] T060 [P] [US8] Delete `packages/editor/` directory entirely
- [ ] T061 [P] [US8] Delete `packages/templates/` directory entirely
- [ ] T062 [US8] Clean workspace references — remove entries from `pnpm-workspace.yaml` (if listed), remove from `turbo.json` pipeline (if listed), remove from any `tsconfig.json` `references` arrays
- [ ] T063 [US8] Clean import references — remove from `apps/editor/package.json` deps, remove `jsxImportSource` from `apps/editor/tsconfig.json` and `apps/render/tsconfig.json`, remove from `packages/lumen/src/index.ts` capability manifest requires, remove from `packages/lumen/src/contracts/fixtures/package-capability-manifest.v1.json`
- [ ] T064 [US8] Verify clean build — run `pnpm install`, `pnpm build`, `pnpm typecheck` from workspace root, confirm no errors referencing removed packages. Grep for `lumen-jsx`, `@lumiscia/editor`, `@lumiscia/templates` across workspace to confirm zero matches.

**Checkpoint**: Deprecated packages fully removed. Workspace builds cleanly.

---

## Phase 11: Polish & Cross-Cutting Concerns

**Purpose**: Quality improvements affecting multiple user stories

- [ ] T065 [P] Add editor styles in `apps/editor/src/styles/globals.css` — Tailwind v4 setup, React Flow theme overrides, node color coding by category, handle styling by port type (RasterFrame vs Vector)
- [ ] T066 [P] Add error boundary and loading states to `apps/editor/src/App.tsx` — WASM load failure message, composition parse error display, graceful degradation when WebCodecs unavailable
- [ ] T067 Run `pnpm lint --write` across all modified files
- [ ] T068 Run quickstart.md validation — follow `specs/003-lumen-wasm-node-editor/quickstart.md` steps on a clean checkout and confirm they work

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — can start immediately
- **Foundational (Phase 2)**: Depends on Setup completion — BLOCKS all user stories
- **US1 (Phase 3)**: Depends on Phase 2 (needs JSON delegate types)
- **US2 (Phase 4)**: Depends on US1 (media store feeds WASM renderer)
- **US3 (Phase 5)**: Depends on Phase 2 (needs node registry + graph utils). Can run in PARALLEL with US1+US2 (different packages)
- **US4 (Phase 6)**: Depends on US1, US2, US3 (connects editor to renderer)
- **US5 (Phase 7)**: Depends on US3, US4 (needs editor + preview working)
- **US6 (Phase 8)**: Depends on US3 (needs base node editor). Can run in PARALLEL with US4, US5
- **US7 (Phase 9)**: Depends on US3 (needs editor graph). Can run in PARALLEL with US4, US5
- **US8 (Phase 10)**: Depends on Phase 2 only. Can run in PARALLEL with US1-US7 (removes separate packages)
- **Polish (Phase 11)**: Depends on all desired user stories being complete

### Parallel Opportunities

```
Phase 1 (Setup)
    ↓
Phase 2 (Foundational)
    ↓
┌───────────────────┬───────────────────┬──────────────┐
│ Phase 3 (US1)     │ Phase 5 (US3)     │ Phase 10     │
│ Phase 4 (US2)     │ Phase 8 (US6)     │ (US8)        │
│ (sequential)      │ Phase 9 (US7)     │              │
│                   │ (sequential)      │              │
└────────┬──────────┴────────┬──────────┴──────────────┘
         └────────┬──────────┘
                  ↓
          Phase 6 (US4: Preview)
                  ↓
          Phase 7 (US5: Keyframes)
                  ↓
          Phase 11 (Polish)
```

### Within Each User Story

- Tests (if included) MUST be written and FAIL before implementation
- Types/interfaces before implementations
- Core logic before UI components
- Story complete before moving to next priority
- Commit after each task or logical group
- Stop at any checkpoint to validate story independently

---

## Notes

- [P] tasks = different files, no dependencies
- [Story] label maps task to specific user story for traceability
- Each user story should be independently completable and testable
- Verify new tests fail before implementation and pass after implementation
- Commit after each task or logical group
- Stop at any checkpoint to validate story independently
- Avoid: vague tasks, same file conflicts, cross-story dependencies that break independence
