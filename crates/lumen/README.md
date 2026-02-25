# lumen renderer implementation report

This crate currently provides a scaffolded renderer architecture with concrete module boundaries, trait contracts, and style-aware clip draw paths.

## crate layout

- `src/render/context.rs`
  - Owns frame-level runtime data and the mutable drawing state.
- `src/render/backend/*`
  - Backend trait + concrete backend modules (`software`, `metal`, `vulkan`).
- `src/clip/*`
  - Clip model + draw implementations.
- `src/clip/style/*`
  - Style property model and style resolution.
- `src/media.rs`
  - Media resolver and media store traits used by media clips.
- `src/expr/*`
  - Expression model scaffolding.
- `src/dependency/*`
  - Expression dependency scaffolding.

## renderer context internals

`RendererContext` is the mutable rendering state container. It includes:

- `width`, `height`
- `frame_rate` (`Rational`)
- `clear_color`
- `surface` (main `skia_safe::Surface`)
- `overlay_surface` (secondary `skia_safe::Surface`)
- `media_store: Option<Box<dyn MediaStore>>`

### renderer context lifecycle methods

- `new(width, height, frame_rate)`
  - Allocates raster Skia surfaces for main + overlay targets.
  - Fails with `RendererContextError::SurfaceCreation` if either allocation fails.
- `canvas()` / `overlay_canvas()`
  - Returns mutable canvas references sourced from each surface.
- `clear()`
  - Clears main surface with `clear_color`.
  - Clears overlay with transparent black.
- `set_media_store(...)` and `media_store_mut()`
  - Injects and exposes media resolvers for draw-time media lookups.

## backend contract

`RenderBackend` defines one method:

- `render_frame(renderer_ctx, frame_ctx, provider) -> Result<Vec<u8>, RenderError>`

`FrameProvider` remains available for backend-specific frame sourcing.

`RenderError` currently includes:

- `Unsupported`
- `MissingSource`
- `NotInitialized`
- `PixelReadback`

`backend/mod.rs` also provides shared helpers:

- `pixel_len(width, height)` with overflow protection.
- `read_surface_rgba(renderer_ctx)`
  - Reads RGBA8888 pixels from `renderer_ctx.surface`.
  - Uses unpremultiplied alpha.
  - Returns `PixelReadback` on failure.

## concrete backend modules

### software backend

`SoftwareRenderBackend`:

- Calls `renderer_ctx.clear()`
- Returns readback via `read_surface_rgba(...)`

This is currently the concrete execution path.

### metal backend

`MetalRenderBackend`:

- Implements `RenderBackend`.
- Delegates rendering to internal `SoftwareRenderBackend` fallback.

### vulkan backend

`VulkanRenderBackend`:

- Implements `RenderBackend`.
- Delegates rendering to internal `SoftwareRenderBackend` fallback.

## clip model and draw pipeline

### clip trait

Each clip implements `Clip`:

- `meta() -> &ClipMeta`
- default helpers: `id()`, `start()`, `end()`, `contains_frame(frame)`
- `draw(frame, frame_ctx, renderer_ctx) -> Result<(), RenderError>`

`ClipType` enum dispatches both `meta()` and `draw(...)` to concrete clip types.

### clip meta

Shared per-clip metadata:

- `id: Option<String>`
- `start_frame: u32`
- `end_frame: u32`

### shared base-style draw wrapper

`clip::draw_with_base_style(...)` performs standardized style application around clip-local geometry drawing. It resolves and applies:

- `visible` gate
- `opacity`
- `blend_mode`
- `blur` (currently modeled through alpha attenuation)
- `transform.translate`
- `transform.scale`
- `alignment` offsets (frame-size relative)
- optional `shadow` pass (offset + alpha attenuation)

The closure passed into this wrapper performs clip-specific geometry drawing.

## style system details

`StyleProperty<T>` supports:

- `Value(StyleValue<T>)`
- `Sequence(Sequence<T>)`

`StyleValue<T>` supports:

- `Literal(T)`
- `Expression(StyleExpression<T>)`

Current generic value helpers:

- `resolve_style_value(...)`
- `resolve_style_value_or(...)`

Current base style resolution:

- `resolve_base_style(...) -> ResolvedBaseStyle`
- Includes resolved shadow payload in `ResolvedShadowStyle`

## clip implementations

### group clip

- Holds `style: BaseStyle` and `children: Vec<ClipType>`.
- Uses `draw_with_base_style(...)`.
- Draws each child in-order while propagating errors.

### layout clip

- Holds `style: BaseStyle` + `TaffyTree` + optional root node.
- `compute_layout(...)` delegates to Taffy when root exists.
- Draw currently renders diagnostic bounds/marker under style wrapper.

### shape clip

`ShapeKind` variants:

- `Rectangle(RectStyle)`
- `Ellipse(EllipseStyle)`
- `Polygon(PolygonStyle)`

Each variant draws geometry (rect/oval/path) under base style wrapper. Polygon uses `PathBuilder` + `detach()`.

### text clip

- Uses `TextStyle { base: BaseStyle }`.
- Draws placeholder text box + baseline under style wrapper.

### media clips

#### image clip

- Uses base style wrapper.
- Attempts media lookup via `renderer_ctx.media_store_mut().get_image_resolver(source)`.
- If resolver is present, uses resolver dimensions.
- If missing, falls back to placeholder dimensions.

#### video clip

- Uses base style wrapper.
- Requires media store and video resolver.
- Returns `RenderError::MissingSource("video:<source>")` if unavailable.
- Calls `resolve_frame(frame)` on resolver, uses resolver dimensions, then draws placeholder body + progress strip.

## media abstraction

- `MediaStore` is object-safe (`Box<dyn ...>` return values).
- `get_image_resolver(&mut self, &str)` / `get_video_resolver(&mut self, &str)` are mutable and late-bound.

## expression + dependency scaffolding status

Expression and dependency modules are present with data models and API stubs, but evaluation/parsing/order resolution are not fully implemented yet.

## known limitations in current renderer implementation

- No scene/timeline orchestration object yet that owns clips and invokes backend draw traversal.
- Backends currently clear + readback; they do not own a full clip graph renderer loop.
- Text rendering is placeholder geometry, not glyph shaping/rasterization.
- Image/video drawing uses resolver metadata, not decoded pixel upload/blitting yet.
- Base style blur/shadow application is scaffold-level approximation.
- Expressions and keyframe interpolation are not fully evaluated.
