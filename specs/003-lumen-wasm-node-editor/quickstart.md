# Quickstart: Lumen WASM + Node Editor

**Branch**: `003-lumen-wasm-node-editor`

## Prerequisites

- Node.js 20+
- pnpm 9+
- Rust toolchain (for building lumen-wasm)
- Emscripten SDK (for WASM compilation)

## Getting Started

```bash
# 1. Install dependencies
pnpm install

# 2. Build WASM artifacts (required for preview rendering)
pnpm wasm:build

# 3. Start the editor app in dev mode
pnpm --filter @lumiscia/editor-app dev
```

The editor opens at `http://localhost:5173`.

## Project Structure

```
packages/lumen/src/
├── canvas/              # WASM bindings + preview renderer (existing, adapted)
│   ├── wasm.ts          # LumenWasmBindings — low-level WASM module interface
│   ├── renderer.ts      # LumenPreviewRenderer — high-level render API
│   ├── media.ts         # MediaStore + VideoFrameDecoder (mediabunny)
│   ├── surface.ts       # RenderSurface — canvas pixel presentation
│   └── types.ts         # Shared types (FrameImage, FrameProvider, etc.)
├── json-delegate/       # NEW: JSON delegate TS types matching Rust schema
│   ├── types.ts         # JsonComposition, JsonNode, JsonConnection, etc.
│   ├── serialize.ts     # Graph → JsonComposition serializer
│   ├── deserialize.ts   # JsonComposition → Graph deserializer
│   └── validate.ts      # Structural validation (cycles, required nodes, port types)
└── index.ts             # Package exports

apps/editor/src/
├── main.tsx             # App entry point
├── App.tsx              # Root component with ReactFlowProvider
├── store/               # NEW: Zustand stores
│   ├── composition.ts   # Graph state (nodes, edges, properties)
│   ├── timeline.ts      # Playback state (frame, playing, fps)
│   └── preview.ts       # Preview renderer state
├── nodes/               # NEW: React Flow custom node components
│   ├── registry.ts      # NodeTypeDef registry + NodeTypes map
│   ├── base-node.tsx    # Shared node wrapper (handles, selection, label)
│   ├── source-nodes.tsx # Shape, MediaIn, SolidColor, Text
│   ├── process-nodes.tsx # Transform, Crop, Resize, Blur, Shadow, etc.
│   ├── composite-nodes.tsx # Merge, Boolean, Switch
│   └── terminal-nodes.tsx # MediaOutput, Memo, FrameHold
├── components/          # UI components
│   ├── node-palette.tsx # Draggable node type list
│   ├── inspector/       # Property editor panel
│   │   ├── inspector.tsx
│   │   ├── property-fields.tsx # Per-type input controls
│   │   └── keyframe-editor.tsx
│   ├── preview-panel.tsx # Canvas + play/pause + stats
│   └── timeline-bar.tsx  # Frame scrubber
├── preview/             # Preview worker integration
│   ├── worker.ts        # Web Worker (renders via WASM)
│   └── use-preview.ts   # Hook connecting worker to React
└── lib/                 # Utilities
    ├── graph-utils.ts   # Cycle detection, validation helpers
    ├── id-generator.ts  # Stable unique ID generation
    └── serialization.ts # Import/export JSON delegate
```

## Key Workflows

### Adding a New Node Type

1. Add the node kind definition in `apps/editor/src/nodes/registry.ts`
2. Add the node component in the appropriate `*-nodes.tsx` file
3. Add property fields in `apps/editor/src/components/inspector/property-fields.tsx`
4. Ensure the JSON delegate type in `packages/lumen/src/json-delegate/types.ts` matches the Rust schema

### Testing JSON Delegate Output

```bash
# Run vitest for the lumen package
pnpm --filter @lumiscia/lumen test

# Validate a JSON delegate file against the WASM renderer
# (loads into WASM, renders frame 0, checks for errors)
pnpm --filter @lumiscia/editor-app test
```

### Building WASM

```bash
# Full WASM build (requires emscripten)
pnpm wasm:build

# Verify WASM artifacts exist
ls packages/lumen/public/lumen-wasm/
# Should contain: lumen_wasm.js, lumen_wasm.wasm
```

## Key Decisions

| Decision | Rationale |
|----------|-----------|
| `@xyflow/react` (React Flow v12) for node editor | Mature, typed, built-in cycle prevention utils |
| Zustand for state management | Already in project, lightweight, works well with React Flow |
| Web Worker for WASM rendering | Existing pattern in codebase, prevents UI blocking |
| JSON delegate as canonical format | Matches Rust schema exactly, consumed by both WASM and native |
| mediabunny for video decoding | Already integrated, uses WebCodecs, proven in production |
