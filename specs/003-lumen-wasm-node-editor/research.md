# Research: Lumen WASM + Node Editor Rework

**Branch**: `003-lumen-wasm-node-editor`
**Date**: 2026-02-22

## R1: React Flow Library — Version and API Patterns

**Decision**: Use `@xyflow/react` (React Flow v12+), the renamed package for React Flow.

**Rationale**: React Flow v12 is the current major version. The package was renamed from `reactflow` to `@xyflow/react`. It provides:
- Custom node components via `NodeTypes` map — each node type is a React component receiving `NodeProps<T>`
- Typed handles with `<Handle type="source"|"target" position={...} id={portName} />`
- `isValidConnection` callback for connection validation (type checking, cycle prevention)
- Built-in `getOutgoers` utility for cycle detection
- TypeScript-first with generic node/edge types
- Viewport-aware rendering (only renders visible nodes) for large graphs

**Alternatives considered**:
- **Rete.js**: More opinionated, heavier framework. React Flow is lighter and more widely adopted (40k+ GitHub stars vs 10k).
- **Custom canvas-based editor**: Too much effort for this phase. React Flow provides all needed primitives.
- **Litegraph.js**: Canvas-based, not React-native. Poor TypeScript support.

### Key API Patterns

**Custom Node Types**: Define per `NodeKind` — e.g., `SolidColorNode`, `TransformNode`, etc. Each exposes handles matching the Rust port definitions.

**Connection Validation**: Use `isValidConnection` prop on `<ReactFlow>`:
1. Check port type compatibility (RasterFrame↔RasterFrame, Vector↔Vector)
2. Check cycle prevention via `getOutgoers` traversal
3. Check single-connection-per-input (one source per target handle)

**Cycle Detection**: Use `getOutgoers` from `@xyflow/react` with recursive DFS from the target node. If the source node is reachable from the target, the connection would create a cycle.

**State Management**: `@xyflow/react` provides `useNodesState` and `useEdgesState` hooks, but for a complex editor, zustand is preferred (already in the project). Store nodes, edges, and composition metadata in a zustand store, sync to React Flow.

## R2: Off-Main-Thread WASM Rendering

**Decision**: Use the existing Web Worker + OffscreenCanvas pattern already established in `apps/editor/src/preview-worker.ts`.

**Rationale**: The codebase already has a mature, working implementation:
- `preview-worker.ts` runs as a Web Worker
- Main thread transfers an `OffscreenCanvas` via `postMessage` with `Transferable`
- Worker loads WASM module, creates `LumenPreviewRenderer`, renders to the OffscreenCanvas
- Message protocol: `init` (transfer canvas) → `setProject` → `render` (frame requests) → `render-result` / `render-error`
- Render lock prevents concurrent renders
- Trace collection for performance monitoring

**The existing pattern handles**:
- WASM module initialization in worker context
- Project loading/unloading lifecycle
- Media store coordination (image loading, video decoding via mediabunny)
- Display renderer (direct to OffscreenCanvas) vs offscreen renderer (for blob export)
- Error propagation back to main thread

**Adaptation needed**: The existing worker uses the old `Project` type (layers/clips model from `preview-types.ts`). The new worker will use the JSON delegate format (`JsonComposition`) directly. The core rendering loop is the same.

**Alternatives considered**:
- **Main thread rendering**: Blocks UI during frame renders. Unacceptable for interactive editing.
- **SharedArrayBuffer + WASM threading**: Requires COOP/COEP headers, not widely deployed. Out of scope per spec assumptions.
- **WebGPU rendering**: The WASM module uses Skia CPU rendering. GPU path is out of scope per spec 002.

## R3: Node Port Definitions (Extracted from Rust Crate)

**Decision**: Define TypeScript port metadata matching the Rust `InputPortDef`/`OutputPortDef` exactly.

Complete extraction from `crates/lumen/src/node/*.rs`:

| Node Type | Inputs | Outputs | Properties |
|-----------|--------|---------|------------|
| **Shape** | *(none)* | vector (Vector) | geometry: ShapeGeometry |
| **ShapeRenderer** | vector (Vector, req) | output (RasterFrame) | fill_color: [u8;4], stroke_color: [u8;4], stroke_width: f32, fill_enabled: bool, stroke_enabled: bool |
| **MediaIn** | *(none)* | output (RasterFrame) | kind: MediaInKind (Image{source} \| Video{source, range?, speed, loop_mode}) |
| **SolidColor** | *(none)* | output (RasterFrame) | color: [u8;4], width?: u32, height?: u32 |
| **Text** | *(none)* | output (RasterFrame) | content: String, font_family: String, font_size: f32, font_weight: u16, font_style, max_width?: f32, color: [u8;4], alignment |
| **Transform** | source (RasterFrame, req) | output (RasterFrame) | scale_x: f32, scale_y: f32, translate_x: f32, translate_y: f32, rotate: f32, pivot_x: f32, pivot_y: f32, sampling |
| **Crop** | source (RasterFrame, req) | output (RasterFrame) | x: i32, y: i32, width: u32, height: u32 |
| **Resize** | source (RasterFrame, req) | output (RasterFrame) | width: u32, height: u32, mode: ResizeMode, sampling: ResizeSampling |
| **Blur** | source (RasterFrame, req) | output (RasterFrame) | radius: f32 |
| **Shadow** | source (RasterFrame, req) | output (RasterFrame) | offset_x: i32, offset_y: i32, color: [u8;4], blur_radius: f32 |
| **Boolean** | source (RasterFrame, req), mask (RasterFrame, opt), vector (Vector, opt) | output (RasterFrame) | mask_kind: MaskKind, invert: bool |
| **Merge** | base (RasterFrame, req), overlay (RasterFrame, req), mask (RasterFrame, opt) | output (RasterFrame) | blend_mode: BlendMode, opacity: f32 |
| **Switch** | dynamic inputs per map entry (RasterFrame, req) | output (RasterFrame) | map: HashMap<u16, Range<u32>> |
| **FrameHold** | source (RasterFrame, req) | output (RasterFrame) | hold_frame: u32 |
| **MediaOutput** | source (RasterFrame, req) | output (RasterFrame) | *(none)* |
| **Memo** | source (RasterFrame, req) | output (RasterFrame) | cache_id: String, allow_expressions: bool |

**Port types** (from `PortKind` enum):
- `RasterFrame` — RGBA pixel data (bitmap or surface)
- `Surface` — Skia surface reference (not used in JSON delegate path)
- `Vector` — Shape geometry data (used by Shape → ShapeRenderer pipeline)

## R4: JSON Delegate Schema Alignment

**Decision**: TypeScript types in the editor will mirror the Rust `JsonComposition` schema from `crates/lumen/src/json/schema.rs` exactly.

**Key schema structures**:
- `JsonComposition` → root: schema_revision, graph, timeline, render_settings, tracks[], expressions[]
- `JsonGraph` → nodes: JsonNode[], connections: JsonConnection[]
- `JsonNode` → id: u64, kind: JsonNodeKind (tagged enum with `type` field)
- `JsonConnection` → from_node: u64, from_port: JsonPort, to_node: u64, to_port: JsonPort
- `JsonPort` → Named(string) | Indexed(u16) — untagged enum
- `JsonKeyframeTrack` → id, node_id, property_path, value_type, keys[], before/after_extrapolation
- `JsonExpression` → node_id, property_path, source

**Schema revision**: The editor must output a `schema_revision` string. Current revision will be determined by the engine's expected value.

## R5: Package Dependency Impact

**Decision**: Clean removal of `@lumiscia/lumen-jsx`, `@lumiscia/editor`, and `@lumiscia/templates` (templates depends on lumen-jsx).

**Affected files** (from grep):
- `apps/editor/package.json` — remove `@lumiscia/editor` dependency
- `apps/editor/tsconfig.json` — remove `jsxImportSource: @lumiscia/lumen-jsx`
- `apps/editor/src/App.tsx` — remove `editorRegistry` import from `@lumiscia/editor`
- `apps/editor/src/components/editor-panel.tsx` — remove `AnyEditorDefinition` import
- `apps/editor/src/hooks/use-editor-session.ts` — remove `@lumiscia/editor` imports
- `apps/render/tsconfig.json` — remove `jsxImportSource: @lumiscia/lumen-jsx`
- `packages/lumen/src/index.ts` — remove `requires: ['@lumiscia/editor', '@lumiscia/lumen-jsx', '@lumiscia/templates']`
- `packages/lumen/src/contracts/fixtures/package-capability-manifest.v1.json` — remove peer component refs
- `packages/templates/` — entire package depends on `@lumiscia/lumen-jsx`, also removed

**Note**: `apps/editor/` source files will be entirely rewritten for the node editor, so import cleanup there is part of the rewrite, not a separate migration.

## R6: mediabunny Integration Pattern

**Decision**: Reuse the existing `MediaStore` and `VideoFrameDecoder` from `packages/lumen/src/canvas/media.ts` with minimal modifications.

**Rationale**: The existing implementation already:
- Uses `mediabunny` v1.31+ with `Input`, `UrlSource`, `BlobSource`, `CanvasSink`, `ALL_FORMATS`
- Implements LRU caching for images and video frames
- Provides `FrameProvider` interface matching what `LumenPreviewRenderer.renderFrame()` expects
- Handles video source lifecycle (load, decode, dispose)
- Includes tracing support

**What changes**: The `MediaStore` and `VideoFrameDecoder` classes remain as-is in `packages/lumen/src/canvas/`. The preview worker will use them with the new JSON delegate-based project format instead of the old layers/clips format.
