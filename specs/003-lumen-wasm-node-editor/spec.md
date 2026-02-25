# Feature Specification: Lumen WASM + Node Editor Rework

**Feature Branch**: `003-lumen-wasm-node-editor`
**Created**: 2026-02-22
**Status**: Draft
**Input**: User description: "A complete rework of the lumen typescript package to better suit the new lumen architecture. The lumen-wasm part handles keeping a renderer instance available, loading a project into it, rendering frames, providing statistics for rendered frames, and holding the media store. Media store functionality is implemented in JS due to WebCodecs availability (using mediabunny). The editor app is rewritten as a node editor using React Flow, outputting either Rust or JSON delegate format. Includes preview with pause/play functionality. Removes lumen-jsx and packages/editor. Implements keyframes, all nodes, property edits, inputs, outputs, required nodes (like output node), etc."

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Load and Render a Composition via WASM (Priority: P1)

A developer initializes the lumen-wasm module in a browser, loads a composition defined in the JSON delegate format, and renders individual frames to a canvas. The WASM module maintains a persistent renderer instance that can be reused across multiple frame renders without reinitialization. The developer receives rendered RGBA pixel data and can present it on an HTML canvas or offscreen canvas.

**Why this priority**: The WASM renderer is the foundation for all browser-based composition work. Without a working renderer that can load projects and produce frames, the preview system and node editor have nothing to display. Every other feature depends on this capability.

**Independent Test**: Can be fully tested by constructing a minimal JSON delegate composition (SolidColor → MediaOutput), loading it into the WASM renderer, rendering frame 0, and verifying the output pixel data matches the expected solid color at the specified canvas dimensions.

**Acceptance Scenarios**:

1. **Given** a valid JSON delegate composition with a SolidColor node connected to a MediaOutput node, **When** the composition is loaded into the WASM renderer, **Then** a renderer instance handle is returned and remains valid for subsequent frame render calls.
2. **Given** a loaded renderer instance, **When** frame 0 is rendered, **Then** RGBA pixel data is returned with the correct dimensions (width × height × 4 bytes) and the pixel values match the expected output.
3. **Given** a loaded renderer instance, **When** a new composition is loaded, **Then** the previous renderer is properly disposed and the new composition takes its place without memory leaks.
4. **Given** an invalid or malformed JSON delegate payload, **When** loading is attempted, **Then** a structured error is returned with a descriptive message identifying the parsing failure, and no renderer instance is created.
5. **Given** a renderer instance with a multi-node graph, **When** multiple frames are rendered in sequence (frame 0, 1, 2...), **Then** each frame produces correct output and the renderer instance remains valid throughout.

---

### User Story 2 - Decode and Supply Video Frames via JS Media Store (Priority: P1)

A developer uses the JS-side media store (powered by mediabunny and WebCodecs) to decode video sources and supply decoded RGBA frames to the WASM renderer. When the renderer requests media for a given frame, the JS layer decodes the required video frame using WebCodecs, converts it to RGBA pixels, and uploads it to the WASM media store. Images are loaded, decoded to RGBA, and uploaded similarly.

**Why this priority**: Video and image media are core content types for any composition. The WASM renderer cannot access WebCodecs directly (emscripten limitation), so the JS-side media store is the only path for video frame delivery. Without it, only procedural nodes (SolidColor, Shape) can produce output.

**Independent Test**: Can be tested by creating a composition with a MediaIn(Video) node, providing a test video file via the JS media store, rendering a frame, and verifying that the WASM renderer receives and composites the decoded video frame correctly.

**Acceptance Scenarios**:

1. **Given** a composition referencing a video source by ID, **When** frame requirements are queried from the WASM renderer, **Then** the JS layer receives a list of required video source frames (source ID + frame numbers) that need decoding.
2. **Given** a required video frame, **When** the JS media store decodes it using mediabunny/WebCodecs, **Then** the decoded RGBA pixel data is uploaded to the WASM media store and the subsequent render call produces correct composited output.
3. **Given** a composition referencing an image source by ID, **When** the image is loaded and decoded to RGBA pixels, **Then** the image data is uploaded to the WASM media store and persists across multiple frame renders without re-upload.
4. **Given** sequential frame renders of a video composition, **When** previously decoded video frames are still in the JS-side LRU cache, **Then** cached frames are reused without re-decoding.
5. **Given** the JS media store's video frame cache reaches its capacity limit, **When** new frames need decoding, **Then** least-recently-used frames are evicted and the WASM media store's video frames are cleared to maintain bounded memory usage.

---

### User Story 3 - Build a Node Graph in the Editor (Priority: P1)

A developer opens the node editor and constructs a composition by placing nodes from the available node set onto a canvas, connecting node output ports to input ports via drag-and-drop, and configuring node properties through an inspector panel. The editor enforces graph validity rules: connections respect port types, required nodes (MediaOutput) are always present, and cycles are prevented.

**Why this priority**: The node editor is the primary authoring interface. Without the ability to construct and wire a valid node graph, users cannot create compositions at all. This is the central user-facing feature of the rework.

**Independent Test**: Can be tested by opening the editor, adding a SolidColor node and a MediaOutput node, connecting SolidColor's output to MediaOutput's "source" input, and verifying the resulting graph structure is valid and serializable to JSON delegate format.

**Acceptance Scenarios**:

1. **Given** an empty editor canvas, **When** the editor loads, **Then** a MediaOutput node is present by default (it is required and cannot be deleted).
2. **Given** the node palette, **When** a developer drags a node type onto the canvas, **Then** a new instance of that node is created with default property values and unique stable ID.
3. **Given** two compatible nodes, **When** the developer drags from an output port to an input port, **Then** a connection is created and visually rendered as an edge between the nodes.
4. **Given** an attempt to connect two incompatible port types, **When** the drag completes, **Then** the connection is rejected and no edge is created.
5. **Given** an attempt to create a connection that would form a cycle, **When** the drag completes, **Then** the connection is rejected with a visual indication of why.
6. **Given** a node on the canvas, **When** the developer selects it, **Then** the inspector panel displays all configurable properties for that node type with their current values.
7. **Given** a complete valid graph, **When** the developer triggers serialization, **Then** the graph is exported as a valid JSON delegate document matching the schema consumed by the lumen-wasm renderer.

---

### User Story 4 - Preview Composition in Real Time (Priority: P1)

A developer sees a live preview of their composition rendered by the WASM engine as they build and modify the node graph. The preview updates when graph structure or node properties change. Play/pause controls allow scrubbing through the timeline to preview animation at different frames. Render statistics (frame time, resolution) are displayed alongside the preview.

**Why this priority**: Without visual feedback, the node editor is an abstract graph tool with no connection to the rendered output. Real-time preview is what makes the editor usable for composition authoring.

**Independent Test**: Can be tested by creating a SolidColor → MediaOutput graph, verifying the preview canvas shows the solid color, then changing the color property and verifying the preview updates to reflect the new color.

**Acceptance Scenarios**:

1. **Given** a valid node graph, **When** the graph changes (node added, removed, property changed, connection modified), **Then** the preview re-renders the current frame within a perceptible response time.
2. **Given** a composition with animation (keyframed properties), **When** the developer presses play, **Then** the preview renders frames sequentially at the composition's frame rate, advancing through the timeline.
3. **Given** a playing preview, **When** the developer presses pause, **Then** playback stops at the current frame and the preview holds that frame.
4. **Given** a paused preview, **When** the developer scrubs the timeline position, **Then** the preview renders the frame at the new position.
5. **Given** a rendered frame, **When** the developer views the statistics panel, **Then** render time (milliseconds), current frame number, total frames, and output resolution are displayed.
6. **Given** a composition with video or image media sources, **When** the preview renders a frame requiring those sources, **Then** the JS media store decodes and supplies the required frames before rendering completes.

---

### User Story 5 - Edit Keyframe Animation Tracks (Priority: P2)

A developer creates and edits keyframe animation tracks targeting specific properties of specific nodes. Each track specifies a node ID, property path, value type, and a sequence of keyframes with frame positions, values, and interpolation modes. The editor provides UI for adding, removing, and modifying keyframes, and the preview reflects animated values during playback.

**Why this priority**: Keyframe animation is what distinguishes a compositing engine from a static image compositor. It is essential for motion graphics but depends on the node graph and preview being functional first.

**Independent Test**: Can be tested by creating a Transform node with a keyframe track on translate_x (frame 0 → 0, frame 60 → 500, Linear interpolation), playing the preview, and verifying the transformed content moves from left to right over 60 frames.

**Acceptance Scenarios**:

1. **Given** a node with an animatable property, **When** the developer adds a keyframe at frame 0 with value A and frame 60 with value B, **Then** a keyframe track is created targeting that node's property path.
2. **Given** a keyframe track with two keys, **When** the preview renders a frame between the two keys, **Then** the property value is interpolated according to the track's interpolation mode (Step or Linear).
3. **Given** a keyframe track, **When** the developer modifies a keyframe's value or frame position, **Then** the track updates and the preview reflects the change.
4. **Given** a keyframe track, **When** the developer removes a keyframe, **Then** the track updates accordingly; if only one keyframe remains, the property holds that value at all frames.
5. **Given** multiple keyframe tracks on different properties of different nodes, **When** the composition is serialized, **Then** all tracks are included in the JSON delegate output with correct node IDs, property paths, and key data.
6. **Given** a keyframe track with Hold extrapolation, **When** the preview renders a frame before the first key or after the last key, **Then** the nearest key's value is used.

---

### User Story 6 - Configure All Node Types and Their Properties (Priority: P2)

A developer can place and configure every node type from the v1 node set: Shape, ShapeRenderer, MediaIn (Image/Video), SolidColor, Text, Transform, Crop, Resize, Blur, Shadow, Boolean, Merge, Switch, FrameHold, MediaOutput, and Memo. Each node exposes its specific properties in the inspector, and the editor provides appropriate input controls for each property type (color pickers for RGBA, numeric inputs for floats/integers, dropdowns for enums, text fields for strings).

**Why this priority**: Partial node support would limit the compositions users can build. Full coverage of the v1 node set ensures the editor can produce any composition the engine supports.

**Independent Test**: Can be tested by placing each node type on the canvas, verifying its properties appear in the inspector with correct types, modifying each property, and verifying the serialized JSON delegate contains the correct property values.

**Acceptance Scenarios**:

1. **Given** each of the 16 node types in the v1 set, **When** placed on the canvas, **Then** each exposes its defined input ports, output ports, and configurable properties in the inspector.
2. **Given** a Shape node, **When** the developer selects rectangle geometry and sets width/height, **Then** the node's properties reflect the chosen geometry type and dimensions.
3. **Given** a MediaIn node, **When** the developer selects "video" media type and provides a source ID, **Then** the node's properties include source, range, speed, and loop mode fields.
4. **Given** a Merge node, **When** the developer configures blend mode and opacity, **Then** the properties are reflected in both the inspector and the serialized output.
5. **Given** a Switch node, **When** the developer defines frame range mappings, **Then** the switch map is correctly serialized with start/end ranges for each entry.
6. **Given** any node with an RGBA color property, **When** the developer uses the color picker, **Then** all four channels (R, G, B, A) are independently editable and stored as a 4-element array.
7. **Given** a Transform node, **When** the developer sets scale_x, scale_y, translate_x, translate_y, rotate, pivot_x, and pivot_y, **Then** all seven properties are independently configurable and serialized.

---

### User Story 7 - Export Valid JSON Delegate Format (Priority: P2)

A developer exports their composition from the node editor as a JSON delegate document. The exported document conforms to the schema consumed by the lumen-wasm renderer and the native lumen engine. The export includes the complete graph (nodes with IDs and properties, connections with port references), timeline settings, render settings, keyframe tracks, and expressions. The exported JSON can be loaded back into the editor without data loss.

**Why this priority**: The JSON delegate format is the interchange format between the editor and the renderer. If the editor produces invalid JSON, nothing works. Round-trip fidelity ensures the editor is a reliable authoring tool.

**Independent Test**: Can be tested by building a composition with multiple node types, connections, and keyframe tracks, exporting to JSON, loading the JSON back into the editor, and verifying the reconstructed graph matches the original.

**Acceptance Scenarios**:

1. **Given** a complete composition, **When** exported to JSON, **Then** the output contains `schema_revision`, `graph` (nodes + connections), `timeline`, `render_settings`, `tracks`, and `expressions` fields.
2. **Given** an exported JSON document, **When** loaded into the lumen-wasm renderer, **Then** the renderer successfully loads the composition and renders frames without errors.
3. **Given** an exported JSON document, **When** loaded back into the node editor, **Then** all nodes, connections, properties, keyframe tracks, and expressions are reconstructed identically.
4. **Given** a node with stable ID 42, **When** the composition is exported and re-imported, **Then** the node retains ID 42 and all connections referencing it remain valid.
5. **Given** keyframe tracks targeting specific node IDs and property paths, **When** exported, **Then** each track's `node_id`, `property_path`, `value_type`, `keys`, and extrapolation settings are correctly serialized.

---

### User Story 8 - Remove Deprecated Packages (Priority: P3)

The `@lumiscia/lumen-jsx` and `@lumiscia/editor` packages are removed from the repository. All workspace references, imports, and dependencies on these packages are cleaned up. The new node editor app replaces `@lumiscia/editor`'s functionality, and JSX-based composition definitions are replaced by the node editor's graph-based authoring.

**Why this priority**: These packages represent the old architecture and will require extensive rework that is not justified for the current development phase. Removing them reduces confusion and maintenance burden. However, this is a cleanup task that does not block any user-facing functionality.

**Independent Test**: Can be tested by verifying the workspace builds without errors after the packages are removed, and no remaining code references `@lumiscia/lumen-jsx` or `@lumiscia/editor`.

**Acceptance Scenarios**:

1. **Given** the packages `@lumiscia/lumen-jsx` and `@lumiscia/editor`, **When** they are removed from the repository, **Then** no files from these packages remain in the workspace.
2. **Given** the removal, **When** `pnpm install` and `pnpm build` are run, **Then** the workspace resolves dependencies and builds without errors.
3. **Given** any remaining code that previously imported from these packages, **When** the imports are audited, **Then** all references are either removed or migrated to the new equivalents.

---

### Edge Cases

- What happens when the WASM module fails to load (network error, corrupt binary)? The system reports a descriptive error to the user and does not silently fail or hang.
- What happens when a video source referenced by a composition is unavailable or undecodable? The media store reports the failure per-source; the renderer uses a fallback (transparent frame) for that source and continues rendering the rest of the composition.
- What happens when the user attempts to delete the required MediaOutput node? The editor prevents deletion and displays a message explaining the node is required.
- What happens when the user pastes a malformed JSON document into the editor's import function? The editor validates the JSON against the expected schema and reports specific validation errors without crashing.
- What happens when a composition has hundreds of nodes? The node editor remains responsive through viewport-aware rendering and the preview prioritizes the current frame without blocking the UI thread.
- What happens when the user creates a connection and then deletes the source node? The connection is automatically removed along with the node.
- What happens when a keyframe track references a node ID that no longer exists in the graph? Orphaned tracks are detected during serialization validation and reported as warnings; they are excluded from the export.
- What happens when two keyframes in a track have the same frame position? The editor prevents duplicate frame positions within a single track; the second keyframe replaces the first.
- What happens when the browser tab runs out of memory during video decoding? The JS media store enforces bounded caches (LRU for decoded frames, bounded pool for decode buffers) to prevent unbounded memory growth.

## Requirements *(mandatory)*

### Functional Requirements

#### WASM Renderer Integration

- **FR-001**: System MUST provide a high-level API that initializes the lumen-wasm module, manages renderer instance lifecycle (create, load project, render frame, dispose), and exposes render statistics (frame time, dimensions, status).
- **FR-002**: System MUST accept compositions in the JSON delegate format defined by the lumen engine's schema (nodes, connections, timeline settings, render settings, keyframe tracks, expressions).
- **FR-003**: System MUST expose a frame requirements query that, given a frame number, returns the set of image source IDs and video source frame numbers needed before that frame can be rendered.
- **FR-004**: System MUST support loading a new composition into an existing renderer instance, properly disposing the previous composition's state and clearing stale media.

#### JS Media Store

- **FR-005**: System MUST implement a JS-side media store that decodes video frames using mediabunny (WebCodecs) and supplies decoded RGBA pixel data to the WASM media store.
- **FR-006**: System MUST implement a JS-side image loader that fetches, decodes, and uploads image RGBA pixel data to the WASM media store.
- **FR-007**: System MUST maintain bounded LRU caches for decoded video frames (per-source) and decoded images, with configurable capacity limits.
- **FR-008**: System MUST coordinate the render loop: query frame requirements → decode/upload required media → invoke WASM render → present pixels to canvas.
- **FR-009**: System MUST minimize redundant memory copies when transferring decoded pixel data to the WASM renderer, using the most efficient transfer mechanism available.

#### Node Editor

- **FR-010**: System MUST provide a node-based graph editor built with React Flow that supports all 16 node types from the v1 node set: Shape, ShapeRenderer, MediaIn (Image/Video), SolidColor, Text, Transform, Crop, Resize, Blur, Shadow, Boolean, Merge, Switch, FrameHold, MediaOutput, and Memo.
- **FR-011**: System MUST enforce that exactly one MediaOutput node exists in every composition. The MediaOutput node is created by default and cannot be deleted.
- **FR-012**: System MUST display typed input and output ports on each node according to the node type's port definitions. Connections are only permitted between compatible port types.
- **FR-013**: System MUST prevent graph cycles. When a connection would create a cycle, the connection attempt is rejected with user-visible feedback.
- **FR-014**: System MUST provide an inspector panel that displays and allows editing of all configurable properties for the selected node, with input controls appropriate to each property type (color pickers, numeric inputs, dropdowns, text fields, range sliders).
- **FR-015**: System MUST support adding, removing, and editing keyframe animation tracks on animatable node properties, with frame position, value, and interpolation mode (Step, Linear) per keyframe.
- **FR-016**: System MUST support before/after extrapolation settings (Hold, DefaultValue) on keyframe tracks.
- **FR-017**: System MUST support expression assignment to node properties, storing the expression source string associated with the target node ID and property path.
- **FR-018**: System MUST assign stable unique IDs to nodes and keyframe tracks that persist through save/load cycles and do not depend on array position.

#### Preview System

- **FR-019**: System MUST provide a live preview panel that renders the current composition using the WASM renderer, updating when the graph structure or node properties change.
- **FR-020**: System MUST provide play/pause controls and a timeline scrubber for navigating through frames of animated compositions.
- **FR-021**: System MUST display render statistics: frame render time (milliseconds), current frame number, total frame count, and output resolution.
- **FR-022**: System MUST perform rendering without blocking the editor UI, ensuring the user can continue editing while frames are being rendered.

#### Serialization and Export

- **FR-023**: System MUST serialize the node graph to the JSON delegate format consumed by the lumen engine, including all nodes (with IDs, types, and properties), connections (with port references), timeline settings (fps, duration_frames), render settings (width, height, background_color), keyframe tracks, and expressions.
- **FR-024**: System MUST deserialize a JSON delegate document back into the editor's internal graph representation, reconstructing all nodes, connections, properties, keyframe tracks, and expressions.
- **FR-025**: System MUST validate the JSON delegate output: all node IDs referenced by connections and keyframe tracks exist in the graph, all required ports are connected, and the graph contains exactly one MediaOutput node.

#### Package Cleanup

- **FR-026**: System MUST remove the `@lumiscia/lumen-jsx` package and all workspace references to it.
- **FR-027**: System MUST remove the `@lumiscia/editor` package and all workspace references to it.

### Security and Boundary Requirements *(mandatory)*

- **SR-001**: System MUST validate all JSON input during deserialization. Malformed, schema-violating, or excessively large payloads are rejected with structured diagnostics. JSON parsing errors do not propagate as uncaught exceptions.
- **SR-002**: System MUST validate media source references before attempting to load them. Invalid URLs or source IDs produce descriptive errors, not uncaught network failures.
- **SR-003**: System MUST ensure expression source strings stored in the editor are treated as data, not executable code, on the JS side. Expression evaluation occurs only within the WASM renderer's sandboxed expression engine.
- **SR-004**: System MUST enforce bounded memory usage for decoded media (LRU caches with configurable limits) and WASM heap allocations (dispose renderer and media store when no longer needed).

### Operational Requirements *(mandatory)*

- **OR-001**: System MUST produce structured error messages for all failure categories: WASM load failure, project parse failure, render failure, media decode failure, and serialization validation failure. Each error includes sufficient context (source ID, frame number, node ID) for debugging.
- **OR-002**: System MUST implement bounded behavior for the render loop: if a frame render exceeds a configurable timeout, the frame is skipped and the next frame is attempted during playback.
- **OR-003**: System MUST properly dispose WASM resources (renderer handles, media store handles, scratch memory) when the editor component unmounts or when a new composition is loaded.

### Key Entities

- **Composition**: The root document containing a node graph, timeline settings, render settings, animation tracks, and expressions. Serialized as a JSON delegate document.
- **Node**: A graph element with a stable numeric ID, a kind (one of 16 v1 types), typed input/output ports, and configurable properties specific to its kind.
- **Connection**: A directed link from one node's output port to another node's input port, forming the evaluation graph. Serialized with from_node, from_port, to_node, to_port.
- **KeyframeTrack**: Animation data targeting a specific (node_id, property_path) pair, containing a sorted sequence of keyframes with frame positions, values, interpolation modes, and extrapolation settings.
- **Expression**: A source string associated with a (node_id, property_path) pair, evaluated at render time by the WASM engine's expression evaluator.
- **MediaStore**: A two-layer construct — JS-side media store handles video/image decoding (using mediabunny/WebCodecs), while the WASM-side in-memory store holds uploaded RGBA pixel data for the renderer.
- **RenderSession**: The active state of a loaded composition in the WASM renderer, including the renderer handle, media store handle, and associated render statistics.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: A developer can construct a composition with at least 10 interconnected nodes in the editor, serialize it to JSON, load it into the WASM renderer, and render a frame producing correct pixel output — all within a single browser session with no server-side processing.
- **SC-002**: The node editor supports all 16 node types from the v1 set, each with its full set of configurable properties. No node type is stubbed or partially implemented.
- **SC-003**: JSON delegate round-trip fidelity: a composition exported from the editor can be imported back with zero data loss — all nodes, connections, properties, keyframe tracks, and expressions match the original.
- **SC-004**: Preview frame renders complete within 200ms for compositions with up to 20 nodes at 1080p resolution, maintaining interactive responsiveness during graph editing.
- **SC-005**: Video compositions with a single 1080p video source achieve at least 15fps preview playback through the JS media store and WASM renderer pipeline.
- **SC-006**: The editor UI remains responsive (no frame drops in the editor chrome) while the WASM renderer processes frames, demonstrating successful off-main-thread rendering.
- **SC-007**: Memory usage for the JS media store stays bounded: the video frame LRU cache never exceeds its configured limit, and the WASM media store is cleared when compositions change.
- **SC-008**: The exported JSON validates against the lumen engine's JSON delegate schema and can be consumed by both the WASM renderer and the native lumen CLI (`lumen-local`) without modification.

## Assumptions

- The lumen-wasm crate's emscripten-based C ABI is the primary interface for browser rendering. No wasm-bindgen or wasm-pack migration is planned for this phase.
- mediabunny v1.31+ provides the WebCodecs-based video decoding pipeline. No alternative video decoder is considered for this phase.
- The JSON delegate format defined in `crates/lumen/src/json/schema.rs` (JsonComposition) is the canonical serialization format. The editor does not define an alternative schema.
- React Flow is the node editor library. The editor is a React SPA built with Vite.
- The existing `packages/lumen/src/canvas/media.ts` MediaStore and VideoFrameDecoder implementations serve as the baseline for the new JS media store, reusing the LRU cache, frame cache, and mediabunny integration patterns.
- The preview system targets browser environments with WebCodecs support (Chrome 94+, Edge 94+, Safari 16.4+). Older browsers are not supported.
- Rust code generation from the node editor is a stretch goal for this phase. JSON delegate output is the primary and required export format.
- Single-threaded WASM rendering (no SharedArrayBuffer/threading) is assumed for this phase.
- The editor app lives in `apps/editor/` (replacing the existing Vite+React app) and depends on `@lumiscia/lumen` for the WASM/canvas integration.
- Audio is out of scope for this phase.
