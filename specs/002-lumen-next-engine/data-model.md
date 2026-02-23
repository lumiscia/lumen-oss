# Data Model: Lumen/Next Compositing Engine

**Feature**: 002-lumen-next-engine
**Date**: 2026-02-23

## Core Entities

### Composition

Root renderable unit. One composition = one renderable graph + timeline + animation.

| Field | Type | Description |
|---|---|---|
| graph | Graph | Node graph with connections |
| timeline | TimelineSettings | fps, duration_frames |
| render_settings | RenderSettings | width, height, background_color |
| tracks | Vec\<KeyframeTrack\> | Animation tracks targeting node properties |
| metadata | Option\<CompositionMetadata\> | Editor-only, not used in rendering |

**Validation**: Graph must pass all validation checks before rendering. Tracks must reference valid (node_id, property_path) pairs.

---

### TimelineSettings

| Field | Type | Description |
|---|---|---|
| fps | f32 | Frames per second |
| duration_frames | u32 | Total composition length in frames |

**Derived**: `time_seconds(frame) = frame as f64 / fps as f64`

---

### RenderSettings

| Field | Type | Description |
|---|---|---|
| width | u32 | Composition width in pixels |
| height | u32 | Composition height in pixels |
| background_color | [u8; 4] | RGBA8 premultiplied clear color |

---

### Graph

Directed acyclic graph of nodes and connections.

| Field | Type | Description |
|---|---|---|
| nodes | HashMap\<NodeId, Node\> | All nodes keyed by stable ID |
| connections | Vec\<Connection\> | Directed edges between ports |

**Invariants**:
- No cycles
- Exactly one MediaOutput node targeted per render request
- All required inputs connected or node-defined default exists

---

### Node

| Field | Type | Description |
|---|---|---|
| id | NodeId | Stable identifier (never array-position-dependent) |
| kind | NodeKind | Enum variant determining behavior and ports |
| properties | NodeProperties | Kind-specific property values |
| metadata | Option\<NodeMetadata\> | Editor-only position, label, etc. |

---

### NodeId

Newtype: `NodeId(u64)`. Derives `Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Display`.

---

### TrackId

Newtype: `TrackId(u64)`. Same derives as NodeId.

---

### NodeEval (trait — compile-time contract)

Every node struct must implement this trait. It is NOT used for `dyn` dispatch — it exists solely for compile-time enforcement of the node contract.

| Method | Signature | Description |
|---|---|---|
| input_port_defs | (&self) -> &'static [InputPortDef] | Static port descriptors for validation |
| output_port_defs | (&self) -> &'static [OutputPortDef] | Static port descriptors for validation |
| evaluate | (&self, inputs: &NodeInputs, ctx: &mut RenderContext) -> Result\<PortValue, LumenError\> | Evaluate node, producing output from inputs |

---

### NodeKind (enum, #[non_exhaustive])

Each variant wraps a concrete struct defined in its own file under `node/`. Dispatch is via exhaustive `match` on the enum, delegating to each struct's `NodeEval::evaluate` impl.

| Variant | Struct | Inputs | Output | Description |
|---|---|---|---|---|
| Shape | shape::Shape | — | Vector | Rectangle, Ellipse, or Polygon geometry |
| ShapeRenderer | shape_renderer::ShapeRenderer | Vector | RasterFrame | Rasterizes vector with fill/stroke |
| MediaIn | media_in::MediaIn | — | RasterFrame | Image or Video source |
| SolidColor | solid_color::SolidColor | — | RasterFrame | Solid color fill |
| Text | text::Text | — | RasterFrame | Text rendering |
| Transform | transform::Transform | RasterFrame | RasterFrame | Scale, rotate, translate |
| Crop | crop::Crop | RasterFrame | RasterFrame | Rectangular crop |
| Resize | resize::Resize | RasterFrame | RasterFrame | Resize with mode/sampling |
| Blur | blur::Blur | RasterFrame | RasterFrame | Gaussian blur |
| Shadow | shadow::Shadow | RasterFrame | RasterFrame | Drop shadow from alpha |
| Boolean | boolean::Boolean | RasterFrame (source) | RasterFrame | Shape/raster mask |
| Merge | merge::Merge | RasterFrame (base), RasterFrame (overlay), RasterFrame? (mask) | RasterFrame | Composite with blend mode |
| Switch | switch::Switch | RasterFrame[] (dynamic) | RasterFrame | Frame-range-based input selector |
| FrameHold | frame_hold::FrameHold | RasterFrame | RasterFrame | Freeze at specified frame |
| MediaOutput | media_output::MediaOutput | RasterFrame | (edge/sink) | Composition output boundary |
| Memo | memo::Memo | RasterFrame (source) | RasterFrame | Cross-session cache boundary |

---

### NodeInputs

Wraps evaluated upstream outputs for consumption by a node's `evaluate` method.

| Field | Type | Description |
|---|---|---|
| ports | HashMap\<&'static str, PortValue\> | Named port values from upstream nodes |

**Accessor methods**: `get_raster(port)`, `get_raster_optional(port)`, `get_vector(port)` — each returns typed `Result` or `Option`, enforcing port type at evaluation time.

---

### PortValue (enum)

| Variant | Contents | Description |
|---|---|---|
| RasterFrame | RasterFrame | Raster image data |
| Vector | VectorData | Shape geometry data |

---

### InputPortDef / OutputPortDef

| Field | Type | Description |
|---|---|---|
| name | &'static str | Port name used for connection matching |
| kind | PortKind | Expected value type |
| optional | bool | (InputPortDef only) Whether the input can be unconnected |

---

### Connection

| Field | Type | Description |
|---|---|---|
| from_node | NodeId | Source node |
| from_port | OutputPort | Source port (named or indexed) |
| to_node | NodeId | Destination node |
| to_port | InputPort | Destination port (named or indexed) |

---

### PortKind (enum)

| Variant | Direction | Description |
|---|---|---|
| RasterFrame | Input/Output | CPU raster bitmap or surface |
| Surface | Input only | Explicit mutable surface request |
| Vector | Input/Output | Shape geometry data |

---

### RasterFrame (enum)

| Variant | Contents | Description |
|---|---|---|
| Bitmap | Arc\<Vec\<u8\>\>, u32, u32 | Immutable RGBA8 pixel data (width, height) |
| Surface | SurfaceRef | Mutable pooled Skia surface |

**Operations**: `dimensions()`, `to_bitmap()`, `promote_to_surface(pool)`.

---

### SurfaceRef (RAII)

| Field | Type | Description |
|---|---|---|
| surface | skia_safe::Surface | The pooled Skia surface |
| pool | Arc\<SurfacePool\> | Back-reference for return on drop |
| width | u32 | Surface width |
| height | u32 | Surface height |

**On drop**: Surface is cleared and returned to pool.

---

### SurfacePool

| Field | Type | Description |
|---|---|---|
| available | Mutex\<HashMap\<(u32, u32), Vec\<skia_safe::Surface\>\>\> | Pooled surfaces keyed by dimensions |

**Operations**: `acquire(width, height) -> SurfaceRef`, `release(surface, width, height)`.
**Growth**: Creates new surface if pool is empty for requested dimensions. Never fails.

---

## Animation Entities

### KeyframeTrack

| Field | Type | Description |
|---|---|---|
| id | TrackId | Stable track identifier |
| node_id | NodeId | Target node |
| property_path | PropertyPath | Stable dot-separated path (e.g., "transform.translate_x") |
| value_type | AnimatableType | Float, Int, Boolean, Color, Vector2, String |
| keys | Vec\<Keyframe\> | Sorted ascending by time_frame, no duplicates |
| before_extrapolation | Extrapolation | Hold or DefaultValue |
| after_extrapolation | Extrapolation | Hold or DefaultValue |

**Validation**: keys sorted, unique time_frame, interpolation mode valid for value_type, target node/property exists.

---

### Keyframe

| Field | Type | Description |
|---|---|---|
| time_frame | u32 | Composition frame |
| value | PropertyValue | Typed value matching track value_type |
| interpolation | InterpolationMode | Step or Linear |

---

### InterpolationMode (enum)

| Variant | Supported Types |
|---|---|
| Step | All types |
| Linear | Float, Int (with rounding), Color (RGBA component-wise) |

---

### Extrapolation (enum)

| Variant | Behavior |
|---|---|
| Hold | Return nearest key value |
| DefaultValue | Return property's static default |

---

## Expression Entities

### Expression (AST)

| Field | Type | Description |
|---|---|---|
| id | ExpressionId | Stable identifier |
| ast | ExprNode | Parsed expression tree |
| references | Vec\<ExpressionReference\> | Node property references used |

**Always pre-parsed**. Never stored as raw string in runtime.

---

### ExprNode (enum, recursive)

| Variant | Description |
|---|---|
| Literal(ExpressionValue) | Number, Boolean, or String literal |
| Binary(Box\<ExprNode\>, BinaryOp, Box\<ExprNode\>) | +, -, *, /, %, >, <, >=, <=, ==, != |
| Unary(UnaryOp, Box\<ExprNode\>) | -, ! |
| Builtin(BuiltinFn, Vec\<ExprNode\>) | min, max, abs, sin, cos, lerp, etc. |
| Global(GlobalVar) | frame, time, fps, width, height |
| NodeProperty(NodeId, PropertyPath) | Reference to another node's property |
| Conditional(Box\<ExprNode\>, Box\<ExprNode\>, Box\<ExprNode\>) | if(cond, then, else) |

---

### ExpressionValue (enum)

| Variant | Type |
|---|---|
| Number | f64 |
| Boolean | bool |
| String | String |

---

### BuiltinFn (enum)

min, max, abs, floor, ceil, round, sin, cos, clamp, lerp, pow, mod, fract, smoothstep, text_height, text_width, uppercase, lowercase

---

## Media Entities

### MediaStore (trait)

| Method | Signature | Description |
|---|---|---|
| get_image_resolver | (&self, source: &str) -> Option\<Box\<dyn ImageResolver\>\> | Get resolver for image source |
| get_video_resolver | (&self, source: &str) -> Option\<Box\<dyn VideoResolver\>\> | Get resolver for video source |

---

### ImageResolver (trait)

| Method | Signature | Description |
|---|---|---|
| id | (&self) -> &str | Source identifier |
| width | (&self) -> u32 | Image width |
| height | (&self) -> u32 | Image height |
| resolve | (&self) -> Result\<Vec\<u8\>, MediaError\> | RGBA8 pixel data |

---

### VideoFrameResolver (trait)

| Method | Signature | Description |
|---|---|---|
| id | (&self) -> &str | Source identifier |
| width | (&self) -> u32 | Video width |
| height | (&self) -> u32 | Video height |
| frame_count | (&self) -> u32 | Total frames |
| resolve_frame | (&self, frame: u32) -> Result\<Vec\<u8\>, MediaError\> | RGBA8 pixel data for frame |

---

## Runtime Entities

### RenderContext

| Field | Type | Description |
|---|---|---|
| frame | u32 | Current composition frame |
| fps | f32 | Composition FPS |
| width | u32 | Render width |
| height | u32 | Render height |
| duration_frames | u32 | Composition duration |
| surface_pool | Arc\<SurfacePool\> | Shared surface pool |
| asset_cache | Arc\<RwLock\<AssetCache\>\> | Shared asset cache |
| node_output_cache | HashMap\<NodeId, RasterFrame\> | Per-frame fan-out cache |
| media_store | Arc\<dyn MediaStore\> | Platform media provider |
| capability_profile | RuntimeCapabilityProfile | Active capabilities |
| cancellation | CancellationToken | For long-running render abort |

---

### RuntimeCapabilityProfile

| Field | Type | Description |
|---|---|---|
| has_image_resolver | bool | Can resolve images |
| has_video_resolver | bool | Can resolve video frames |
| has_threading | bool | Supports multithreaded render |
| sink_types | Vec\<SinkType\> | Available output sinks |

---

## Error Entities

### LumenError (enum, top-level)

| Variant | Inner Type | Description |
|---|---|---|
| GraphValidation | GraphValidationError | Pre-render graph checks |
| Property | PropertyError | Invalid property access/type |
| Expression | ExpressionError | Parse or evaluation failure |
| Media | MediaError | Source loading/decoding failure |
| Render | RenderError | Frame evaluation failure |
| Threading | ThreadingError | Worker/channel failure |
| Sink | SinkError | Output write failure |

All variants carry structured context fields (node_id, frame, property_path, source, etc.) — never just strings.

---

## Entity Relationships

```
Composition
├── Graph
│   ├── Node (HashMap<NodeId, Node>)
│   │   ├── NodeKind (enum variant)
│   │   └── NodeProperties (kind-specific)
│   └── Connection[]
├── TimelineSettings
├── RenderSettings
└── KeyframeTrack[]
    └── Keyframe[]

RenderContext (per-frame)
├── SurfacePool (shared)
├── AssetCache (shared)
├── NodeOutputCache (per-frame)
├── MediaStore (platform-provided)
├── RuntimeCapabilityProfile
└── CancellationToken

RasterFrame
├── Bitmap (Arc<Vec<u8>>)
└── Surface (SurfaceRef → SurfacePool)
```

---

## State Transitions

### Composition Lifecycle

```
Created → Validated → Rendering → Complete
                   ↘ Cancelled
         → ValidationFailed (terminal for this attempt)
```

### RasterFrame Promotion

```
Bitmap → (SurfacePool.acquire + pixel copy) → Surface
Surface → (pixel readback) → Bitmap
```

### Memo Node State

```
Uncached → (render + persist) → Cached
Cached → (subgraph signature mismatch) → Uncached → re-render
Cached → (signature match) → return cached bitmap
Ineligible → (frame-dependent subgraph) → always pass-through
```
