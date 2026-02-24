# Contract: JSON Delegate TypeScript Types

**Branch**: `003-lumen-wasm-node-editor`
**Date**: 2026-02-22

These TypeScript types mirror the Rust `JsonComposition` schema from `crates/lumen/src/json/schema.rs`.
The editor serializes/deserializes these types. The WASM renderer consumes them.

## Root Type

```typescript
interface JsonComposition {
  schema_revision: string
  graph: JsonGraph
  timeline: JsonTimelineSettings
  render_settings: JsonRenderSettings
  tracks: JsonKeyframeTrack[]
  expressions: JsonExpression[]
  metadata?: JsonCompositionMetadata
}

interface JsonCompositionMetadata {
  name?: string
}
```

## Graph Types

```typescript
interface JsonGraph {
  nodes: JsonNode[]
  connections: JsonConnection[]
}

interface JsonNode {
  id: number // u64 — stable unique ID
  kind: JsonNodeKind
}

interface JsonConnection {
  from_node: number
  from_port: JsonPort
  to_node: number
  to_port: JsonPort
}

type JsonPort = string | number // Named(string) | Indexed(u16)
```

## Node Kind Types (tagged union on `type` field)

```typescript
type JsonNodeKind =
  | { type: 'shape'; geometry: JsonShapeGeometry }
  | {
      type: 'shape_renderer'
      fill_color: [number, number, number, number]
      stroke_color: [number, number, number, number]
      stroke_width: number
      fill_enabled: boolean
      stroke_enabled: boolean
    }
  | { type: 'media_in'; kind: JsonMediaInKind }
  | {
      type: 'solid_color'
      color: [number, number, number, number]
      width?: number
      height?: number
    }
  | {
      type: 'text'
      content: string
      font_family: string
      font_size: number
      font_weight: number
      font_style: JsonTextFontStyle
      max_width?: number
      color: [number, number, number, number]
      alignment: JsonTextAlignment
    }
  | {
      type: 'transform'
      scale_x: number
      scale_y: number
      translate_x: number
      translate_y: number
      rotate: number
      pivot_x: number
      pivot_y: number
      sampling: JsonTransformSampling
    }
  | { type: 'crop'; x: number; y: number; width: number; height: number }
  | {
      type: 'resize'
      width: number
      height: number
      mode: JsonResizeMode
      sampling: JsonResizeSampling
    }
  | { type: 'blur'; radius: number }
  | {
      type: 'shadow'
      color: [number, number, number, number]
      blur_radius: number
      offset_x: number
      offset_y: number
    }
  | { type: 'boolean'; mask_kind: JsonMaskKind; invert: boolean }
  | {
      type: 'merge'
      blend_mode: JsonBlendMode
      opacity: number
    }
  | { type: 'switch'; map: Record<string, JsonRange> }
  | { type: 'frame_hold'; hold_frame: number }
  | { type: 'media_output' }
  | {
      type: 'memo'
      cache_id: string
      allow_expressions: boolean
    }
```

## Supporting Enum Types

```typescript
type JsonShapeGeometry =
  | { type: 'rectangle'; width: number; height: number }
  | { type: 'ellipse'; width: number; height: number }
  | { type: 'polygon'; points: [number, number][] }

type JsonMediaInKind =
  | { media_type: 'image'; source: string }
  | {
      media_type: 'video'
      source: string
      range?: JsonRange
      speed: number
      loop_mode: JsonLoopMode
    }

type JsonLoopMode = 'none' | 'repeat' | 'ping_pong'
type JsonBlendMode = 'normal' | 'multiply' | 'screen' | 'overlay' | 'darken' | 'lighten'
type JsonMaskKind = 'alpha' | 'luma'
type JsonResizeMode = 'stretch' | 'fit' | 'fill'
type JsonResizeSampling = 'nearest' | 'bilinear'
type JsonTransformSampling = 'nearest' | 'bilinear'
type JsonTextFontStyle = 'normal' | 'italic' | 'oblique'

interface JsonTextAlignment {
  horizontal: 'left' | 'center' | 'right' | 'justify'
  vertical: 'top' | 'middle' | 'bottom'
}

interface JsonRange {
  start: number
  end: number
}
```

## Timeline and Render Settings

```typescript
interface JsonTimelineSettings {
  fps: number
  duration_frames: number
}

interface JsonRenderSettings {
  width: number
  height: number
  background_color: [number, number, number, number]
}
```

## Animation Types

```typescript
interface JsonKeyframeTrack {
  id: number
  node_id: number
  property_path: string
  value_type: JsonAnimatableType
  keys: JsonKeyframe[]
  before_extrapolation: JsonExtrapolation
  after_extrapolation: JsonExtrapolation
}

interface JsonKeyframe {
  time_frame: number
  value: unknown // type depends on value_type
  interpolation: JsonInterpolationMode
}

type JsonAnimatableType = 'float' | 'int' | 'boolean' | 'color' | 'vector2' | 'string'
type JsonInterpolationMode = 'step' | 'linear'
type JsonExtrapolation = 'hold' | 'default_value'
```

## Expression Types

```typescript
interface JsonExpression {
  node_id: number
  property_path: string
  source: string
}
```

## Preview Worker Protocol

```typescript
// Main thread → Worker
type WorkerInMessage =
  | { type: 'init'; canvas: OffscreenCanvas }
  | { type: 'loadComposition'; composition: JsonComposition; version: number }
  | { type: 'render'; id: number; frame: number; version: number }
  | { type: 'dispose' }

// Worker → Main thread
type WorkerOutMessage =
  | { type: 'ready' }
  | { type: 'render-result'; id: number; frame: number; version: number; durationMs: number }
  | { type: 'render-error'; id: number; version: number; error: string }
```

## Node Port Definition Contract

```typescript
// Used by the editor to render handles and validate connections
interface PortDef {
  name: string
  kind: 'raster_frame' | 'vector'
  optional?: boolean // only for inputs
}

interface NodeTypeDef {
  type: string // matches JsonNodeKind type field
  label: string
  category: 'source' | 'processing' | 'compositing' | 'terminal'
  inputs: PortDef[]
  outputs: PortDef[]
  defaultProperties: Partial<JsonNodeKind>
}
```
