# Data Model: Lumen WASM + Node Editor Rework

**Branch**: `003-lumen-wasm-node-editor`
**Date**: 2026-02-22

## Entity Overview

```
Composition (root)
├── RenderSettings
├── TimelineSettings
├── Graph
│   ├── Node[] (16 kinds)
│   └── Connection[]
├── KeyframeTrack[]
│   └── Keyframe[]
└── Expression[]
```

## Entities

### Composition

The root document. Serialized as JSON delegate format.

| Field | Type | Constraints |
|-------|------|-------------|
| schema_revision | string | Required. Must match engine's expected revision. |
| graph | Graph | Required. Must contain exactly one MediaOutput node. |
| timeline | TimelineSettings | Required. |
| render_settings | RenderSettings | Required. |
| tracks | KeyframeTrack[] | Optional (default: []). |
| expressions | Expression[] | Optional (default: []). |
| metadata | CompositionMetadata? | Optional. |

### RenderSettings

| Field | Type | Constraints |
|-------|------|-------------|
| width | u32 | > 0 |
| height | u32 | > 0 |
| background_color | [u8; 4] | RGBA, each channel 0-255 |

### TimelineSettings

| Field | Type | Constraints |
|-------|------|-------------|
| fps | f32 | > 0, finite |
| duration_frames | u32 | > 0 |

### Graph

| Field | Type | Constraints |
|-------|------|-------------|
| nodes | Node[] | Must contain exactly one node of kind `media_output`. No duplicate IDs. |
| connections | Connection[] | Each connection references valid node IDs and port names. No cycles. |

**Validation rules**:
- Graph must be a DAG (directed acyclic graph)
- Exactly one `media_output` node must exist
- All `to_node`/`from_node` in connections must reference nodes in the graph
- Port names in connections must match the node's defined ports
- Non-optional input ports on nodes reachable from MediaOutput must be connected

### Node

| Field | Type | Constraints |
|-------|------|-------------|
| id | u64 | Unique within the graph. Stable across save/load. |
| kind | NodeKind | Tagged union — see Node Kinds below. |

### NodeKind (tagged enum, discriminated by `type` field)

**Source nodes** (no inputs):

| Kind | Properties |
|------|-----------|
| `shape` | geometry: { type: "rectangle" \| "ellipse" \| "polygon", ... } |
| `media_in` | kind: { media_type: "image" \| "video", source: string, range?, speed?, loop_mode? } |
| `solid_color` | color: [u8;4], width?: u32, height?: u32 |
| `text` | content: string, font_family: string, font_size: f32, font_weight: u16, font_style?, max_width?: f32, color: [u8;4], alignment? |

**Processing nodes** (source → output):

| Kind | Properties |
|------|-----------|
| `shape_renderer` | fill_color: [u8;4], stroke_color: [u8;4], stroke_width: f32, fill_enabled: bool, stroke_enabled: bool |
| `transform` | scale_x: f32, scale_y: f32, translate_x: f32, translate_y: f32, rotate: f32, pivot_x: f32, pivot_y: f32, sampling? |
| `crop` | x: u32, y: u32, width: u32, height: u32 |
| `resize` | width: u32, height: u32, mode: "stretch"\|"fit"\|"fill", sampling: "nearest"\|"bilinear" |
| `blur` | radius: f32 |
| `shadow` | color: [u8;4], blur_radius: f32, offset_x: f32, offset_y: f32 |
| `frame_hold` | hold_frame: u32 |
| `memo` | cache_id: string, allow_expressions: bool |

**Compositing nodes** (multiple inputs):

| Kind | Properties |
|------|-----------|
| `boolean` | mask_kind: "alpha"\|"luma", invert: bool |
| `merge` | blend_mode: "normal"\|"multiply"\|"screen"\|"overlay"\|"darken"\|"lighten", opacity: f32 |
| `switch` | map: { [key: string]: { start: u32, end: u32 } } |

**Terminal node**:

| Kind | Properties |
|------|-----------|
| `media_output` | *(none)* |

### Connection

| Field | Type | Constraints |
|-------|------|-------------|
| from_node | u64 | Must reference existing node ID. |
| from_port | string \| u16 | Named port or indexed port. Must match node's output port. |
| to_node | u64 | Must reference existing node ID. |
| to_port | string \| u16 | Named port or indexed port. Must match node's input port. |

**Validation rules**:
- `from_node` and `to_node` must differ (no self-loops)
- Port kinds must match: RasterFrame↔RasterFrame, Vector↔Vector
- Each input port may have at most one incoming connection
- Output ports may have multiple outgoing connections
- No connection may create a cycle in the graph

### KeyframeTrack

| Field | Type | Constraints |
|-------|------|-------------|
| id | u64 | Unique among all tracks. |
| node_id | u64 | Must reference existing node ID. |
| property_path | string | Must be a valid property path for the node's kind. |
| value_type | "float" \| "int" \| "boolean" \| "color" \| "vector2" \| "string" | Must match the property's type. |
| keys | Keyframe[] | Sorted by time_frame. No duplicate time_frame values. |
| before_extrapolation | "hold" \| "default_value" | Default: "hold" |
| after_extrapolation | "hold" \| "default_value" | Default: "hold" |

### Keyframe

| Field | Type | Constraints |
|-------|------|-------------|
| time_frame | u32 | >= 0 |
| value | JSON value | Type must match track's value_type. |
| interpolation | "step" \| "linear" | |

### Expression

| Field | Type | Constraints |
|-------|------|-------------|
| node_id | u64 | Must reference existing node ID. |
| property_path | string | Must be a valid property path for the node's kind. |
| source | string | Expression source text. Treated as data, not executable code. |

## Port Type Compatibility Matrix

| From \ To | RasterFrame | Vector |
|-----------|-------------|--------|
| RasterFrame | ✅ | ❌ |
| Vector | ❌ | ✅ |

## Editor State (not serialized to JSON delegate)

The editor maintains additional state not part of the composition:

| Field | Purpose |
|-------|---------|
| Node positions (x, y) | React Flow canvas placement |
| Selection state | Currently selected nodes/edges |
| Viewport (zoom, pan) | Canvas view state |
| Playback state | Current frame, playing/paused, playback speed |
| Media source mappings | Source ID → URL/blob for preview |

Node positions are stored as React Flow node data and persist in the editor's own save format, separate from the JSON delegate export (which has no concept of visual layout).

## State Transitions

### Composition Lifecycle
```
Empty → Editing → Valid → Exported
  ↑       ↓
  └── Invalid (validation errors shown inline)
```

### Preview Lifecycle
```
Idle → Loading WASM → Ready → Rendering → Frame Displayed
                        ↑         ↓
                        └── Error (retry on next change)
```

### Playback State Machine
```
Stopped (frame=0) → Playing → Paused (frame=N)
    ↑                  ↓           ↓
    └──────────────────┴───────────┘
```
