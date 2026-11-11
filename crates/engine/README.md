# lumen-engine

`lumen-engine` is the core composition and render orchestration crate. It defines the composition model, node system, media abstractions, JSON support, and the high-level GPU renderer used by the native server, local tools, and WASM bindings.

This crate is experimental. Public APIs, node definitions, schema shapes, and render behavior may change.

## Platform Notes

The main renderer paths require a GPU through `lumen-engine-gpu`/`wgpu`.

Supported native render targets today are:

- Linux with Vulkan, plus optional CUDA/NVENC interop when built with `vulkan,cuda,ffmpeg`.
- macOS with Metal, plus VideoToolbox-oriented media paths when built with `metal,ffmpeg`.

Browser/WASM preview paths are separate and use WebGPU-facing code.

JSON composition parsing is always available. In JSON params, a string starting with `=` is an expression (`"=frame * 2"`); a leading `==` escapes a literal string starting with `=` (`"==wow"` is the text `=wow`).

## Asset references

A source field can name a file in the host's asset store as `asset:<id>`: `media_in`'s `source`,
an audio clip's `source_id`, and a text node's or span's `font_family`. The prefix is a scheme, so
it can't be mistaken for a relative path. Each asset has a kind, image, video, audio or font, and
a reference must name one of the kind its field reads. An id only resolves on the host that issued
it.

A host gives the engine its store as an `AssetResolver`, and its `MediaStore` resolves references
with `media::resolve_asset`:

```rust
pub trait AssetResolver: Send + Sync + Debug {
    fn resolve(&self, id: &str) -> Option<Asset>;
}

pub enum Asset {
    Image(PathBuf),
    Video(PathBuf),
    Audio(PathBuf),
    Font(Vec<PathBuf>), // one family; weight and style choose among its faces
}
```

Fonts load into the text session of the job's store, under the name the composition uses, such
as `asset:<id>` or a `media` alias. Faces that don't carry that name answer to it in place of their
own, so a font asset or a renamed file shapes as asked, and can't be picked by its internal family
name instead of another font of that name.

## Diagnostics

`json::check` reads a composition the way a render would, without rendering it, and returns every
problem it finds as a `Diagnostic`:

```rust
pub struct Diagnostic {
    pub severity: Severity,   // Error or Warning
    pub code: &'static str,   // stable, listed below
    pub path: String,         // JSON pointer, such as /nodes/3/params/opacity; empty for the document
    pub node: Option<NodeId>, // the node, in the graph `path` points into
    pub message: String,      // free-form; don't match on it
}
```

```rust
let fonts = |family: &str| my_fonts.contains(family);
for diagnostic in lumen_engine::json::check(&json, Some(&fonts), Some(&my_assets)) {
    eprintln!("{} at {}: {}", diagnostic.code, diagnostic.path, diagnostic.message);
}
```

`json::parse` fails with the same errors, in a `ParseError` whose `diagnostics()` lists them.
Checking runs in phases: the document's shape, its `functions`, every graph's nodes, every graph's
connections, then the graphs, clips, props, settings and audio. Each phase reports all it finds,
and a phase with errors ends the check, since later ones would mostly repeat them. At most 50
diagnostics are reported.

Passing a `known_font` lookup also checks fonts: each family a text node names as a literal that
the host can't resolve, other than the bundled `Roboto`, is a `font.fallback` warning. Renderers
raise the same warning when shaping falls back, which
`GpuCompositionRenderer::take_diagnostics` returns once per node and family; its path points at
the root or template, not the param.

### Compatibility

- Codes are public API. Messages are not, and may change in any release.
- Warnings never fail a render.
- Clients must accept codes they don't know, treating an unknown error as an error and an unknown
  warning as a warning.
- So a new warning is a patch release. A new error that rejects a composition which renders today
  is a breaking change.

### What only rendering finds

`check` covers what can be told from the composition alone. These fail, or warn, only while
rendering:

- media that is missing or fails to decode;
- expressions that fail on some frames only, such as a division that is only invalid at frame 0;
- `wgsl_shader` code that doesn't compile;
- encoders that won't open;
- fonts that fall back at shaping time, including families set by expressions.

Passing the host's asset store also checks each `asset:` reference a source field holds as a
literal: one the store doesn't have is `asset.not_found`, and one of another kind is
`asset.wrong_kind`. Rendering raises the same errors.

### Codes

| Code                                 | Severity | Found by      | Meaning                                                                                                                                                                                                   |
| ------------------------------------ | -------- | ------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `document.syntax`                    | error    | check         | The text is not JSON.                                                                                                                                                                                     |
| `document.schema_version`            | error    | check         | `lumenSchemaVersion` is not a string, or names a version this engine doesn't read.                                                                                                                        |
| `document.invalid`                   | error    | check         | A key or value outside node params has the wrong shape, is missing or is unknown.                                                                                                                         |
| `expression.function`                | error    | check         | A function in `functions` doesn't parse, or is defined twice or recursively.                                                                                                                              |
| `node.unknown_type`                  | error    | check         | A node's `type` is not a node type.                                                                                                                                                                       |
| `node.invalid_params`                | error    | check         | A node's `params` is not an object.                                                                                                                                                                       |
| `node.unknown_param`                 | error    | check         | A node has a param its type doesn't.                                                                                                                                                                      |
| `node.invalid_param`                 | error    | check         | A param's value is wrong for it, including expressions that don't parse.                                                                                                                                  |
| `graph.duplicate_node_id`            | error    | check         | Two nodes of one graph share an id.                                                                                                                                                                       |
| `connection.invalid_from_port`       | error    | check         | A connection's `from_port` is not `output`.                                                                                                                                                               |
| `connection.missing_source_node`     | error    | check         | A connection or input reads a node that doesn't exist.                                                                                                                                                    |
| `connection.missing_target_node`     | error    | check         | A connection writes to a node that doesn't exist.                                                                                                                                                         |
| `connection.unknown_port`            | error    | check         | A connection writes to an input its node doesn't have.                                                                                                                                                    |
| `connection.input_already_connected` | error    | check         | A single input is connected twice.                                                                                                                                                                        |
| `connection.wrong_kind`              | error    | check         | A connection wires a scene into an input that takes a raster, or a raster into one that takes a scene.                                                                                                    |
| `graph.node_id_mismatch`             | error    | check         | A node is stored under another id. Only builders made in Rust can do this.                                                                                                                                |
| `graph.missing_required_input`       | error    | check         | A required input is not connected.                                                                                                                                                                        |
| `graph.missing_media_output`         | error    | check         | A graph has no `media_output` node.                                                                                                                                                                       |
| `graph.multiple_media_outputs`       | error    | check         | A graph has more than one `media_output` node.                                                                                                                                                            |
| `graph.cycle`                        | error    | check         | Nodes read each other in a cycle.                                                                                                                                                                         |
| `clip.invalid`                       | error    | check         | A clip names a composition that doesn't exist, sets props it doesn't declare or of the wrong kind, plays a composition and a `source` together, or has a negative `duration` other than -1.               |
| `expression.invalid_prop_read`       | error    | check         | `prop()` is used outside a template, names an undeclared prop, or doesn't name it with a string literal.                                                                                                  |
| `settings.invalid`                   | error    | check         | A timeline, canvas or prop default can't render, such as an `fps` of 0.                                                                                                                                   |
| `audio.invalid`                      | error    | check         | Audio tracks or clips are duplicated, unknown, or have invalid timing or volume.                                                                                                                          |
| `composition.cycle`                  | error    | check         | Templates instantiate themselves.                                                                                                                                                                         |
| `font.fallback`                      | warning  | check, render | A text node's font family resolved to no font, so it renders in a fallback.                                                                                                                               |
| `property.invalid_type`              | error    | render        | An expression gave a param a value of the wrong type.                                                                                                                                                     |
| `expression.parse`                   | error    | render        | An expression built at render time doesn't parse.                                                                                                                                                         |
| `expression.evaluate`                | error    | render        | An expression failed to evaluate.                                                                                                                                                                         |
| `media.not_found`                    | error    | render        | A media source isn't available.                                                                                                                                                                           |
| `asset.not_found`                    | error    | check, render | An `asset:` reference names an id the host's asset store doesn't have.                                                                                                                                    |
| `asset.wrong_kind`                   | error    | check, render | An `asset:` reference names an asset of another kind than its field reads, such as a font as an image.                                                                                                    |
| `media.decode`                       | error    | render        | Media or font data couldn't be decoded.                                                                                                                                                                   |
| `media.frame_out_of_range`           | error    | render        | A video frame past the end of its source was asked for.                                                                                                                                                   |
| `audio.unknown_track`                | error    | render        | An audio clip names a track that doesn't exist.                                                                                                                                                           |
| `render.frame_out_of_range`          | error    | render        | A frame past the composition's end was asked for.                                                                                                                                                         |
| `render.node_evaluation`             | error    | render        | A node failed to render, such as text with too many glyphs or a `wgsl_shader` that doesn't compile.                                                                                                       |
| `render.missing_node`                | error    | render        | A node read while rendering doesn't exist.                                                                                                                                                                |
| `render.invalid_output_type`         | error    | render        | A node's output had the wrong type for its reader.                                                                                                                                                        |
| `render.gpu`                         | error    | render        | The GPU failed.                                                                                                                                                                                           |
| `render.too_deeply_nested`           | error    | render        | A node is more than 48 nodes deep, counting through clips. Flatten the chain: put many layers in one `raster_multimerge` instead of chaining `merge`s, and combine consecutive transforms or adjustments. |

## Features

- `ffmpeg`: enable FFmpeg-backed media handling through `lumen-engine-ffmpeg`.
- `image`: enable image decoding helpers.
- `metal`: enable Metal support through `lumen-engine-gpu` and optional FFmpeg interop.
- `vulkan`: enable Vulkan support through `lumen-engine-gpu` and optional FFmpeg interop.
- `cuda`: enable CUDA support through optional FFmpeg interop.

## Development

```bash
cargo check -p lumen-engine
cargo test -p lumen-engine
cargo check -p lumen-engine --features ffmpeg,image
```
