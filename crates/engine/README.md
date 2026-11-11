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

## Document format

A composition may declare `"lumenSchemaVersion": "0.6"`. It equals the Lumen `major.minor` the
document was written for, and an engine reads only its own line: `lumen_engine::SCHEMA_VERSION`
is that value, and a full `major.minor.patch` of the same line is read too. A document from
another line fails with `document.schema_version`, naming both versions. 0.x minor releases are
breaking, so there is no migration between lines. Absent means the engine's own version.

## Colour

Every raster is `Rgba16Float` (`gpu::WORKING_COLOR_FORMAT`): linear light with BT.709 primaries
and premultiplied alpha, where 1.0 is SDR white and colour may go past it. JSON colours, images
and video are sRGB-encoded, and decode to linear light on the way in; `media_output` encodes back
to sRGB once, quantising 8-bit targets with an ordered dither that keeps the mean of flat areas
(`GpuCompositionRenderer::set_output_dither`); a value within a tenth of a code of one keeps it.
So an 8-bit image passed straight through comes out unchanged, and 50% white over black is code
188, half the light, not 128. Float outputs take the working space unchanged.

- Normal merges, opacity, masks, transform and resize sampling, blurs, glow, shadows, motion blur,
  text and shape antialiasing work in linear light. A luma matte is the linear BT.709 luminance;
  the glow threshold compares sRGB-encoded luminance, so 0.5 is half way up the visible range.
- Every other blend mode, `levels`, `hue_saturation`, the `color_grade` LUT, `exposure`'s
  contrast and offset, `displacement_map` (byte 128 is neutral) and `fractal_noise` work on
  sRGB-encoded values, as design tools do.
- Gradients blend their stops encoded (`srgb`, the default) or in linear light (`linear_srgb`).
- Video follows its colour tags. SDR transfers (BT.709, BT.601, BT.2020 SDR, sRGB) decode with
  the sRGB curve. BT.2020 and Display P3 (SMPTE 432) primaries, and DCI-P3 (SMPTE 431) adapted
  to D65, convert to BT.709 in linear light, with colour outside BT.709 clipped at zero. PQ and
  HLG are tone-mapped to SDR as they come in: HLG goes through its OOTF for a 1000-nit display
  (system gamma 1.2), 203 nits is SDR white (1.0), and the ITU-R BT.2390 EETF rolls light off
  from about 88 nits so a 1000-nit peak lands on 1.0, scaling all three channels by the gain it
  gives `max(R, G, B)` so hue is kept. HDR reference white shows at 0.78, sRGB code 229; light
  over 1000 nits clips. BT.2020 constant-luminance video is `media.unsupported`; other
  transfers and primaries read as SDR BT.709 with a `media.color_unsupported` warning.
- Images follow their embedded ICC profile (PNG `iCCP`, JPEG `APP2`, WebP `ICCP`, TIFF and GIF),
  read with `moxcms`; one without a profile is sRGB, and PNG `sRGB`, `gAMA` and `cHRM` chunks
  are not read. Matrix/TRC RGB profiles (sRGB, Display P3, Adobe RGB, ProPhoto, Rec. 2020) and
  grey profiles convert to BT.709 in linear light, colour outside BT.709 clipped at zero and
  kept above 1. An 8-bit image whose profile matches sRGB (every 8-bit code within half a code)
  is read as untagged; one with the sRGB curve in other primaries, such as Display P3, keeps its
  8-bit texture and converts on the GPU. 16-bit PNG and TIFF, float TIFF (read as encoded, as
  before), and 8-bit images with another curve are converted on the CPU to linear half floats,
  8 bytes a pixel. Lookup-table, CMYK, mismatched, oversized (past 1 MiB) or malformed profiles
  read as sRGB with a `media.color_unsupported` warning.
- `wgsl_shader` inputs are linear and premultiplied, its output is `rgba16float`, and its prelude
  has `lumen_srgb_to_linear` and `lumen_linear_to_srgb`.

## Asset references

A source field can name a file in the host's asset store as `asset:<id>`: `media_in`'s `source`,
`model_3d`'s `source`, `render_3d`'s `environment_map`, an audio clip's `source_id`, and a text
node's or span's `font_family`. The prefix is a scheme, so it can't be mistaken for a relative
path. Each asset has a kind, image, video, audio, font, model or environment, and a reference
must name one of the kind its field reads. An id only resolves on the host that issued it.

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

## Supplying media

A host gives a render its media through one method, `MediaStore::resolve(source, kind)`. `source`
is what the composition wrote (a path, an alias, `asset:<id>`, or a font family) and `kind` is the
`AssetKind` the use reads it as. The store answers with a `ResolvedMedia`: an `identity` and a
`MediaResolver`, which holds the kind's resolver trait object (`ImageResolver`,
`VideoFrameResolver`, `AudioResolver`, `FontResolver`, `ModelResolver`, `EnvironmentResolver` or
`LutResolver`).
Video decode settings, such as hardware decode, are the store's own, set when it is made.

The `MediaIdentity` names the bytes. The engine treats equal identities, from any store in the
process, as the same bytes: it decodes, uploads and shapes each identity once and keeps the result
across frames and renders, within its cache budgets. A host must give a source a new identity
when its bytes change, or renders keep showing the old ones. Build it from what the host has:

| Media                                                         | Identity                                                          |
| ------------------------------------------------------------- | ----------------------------------------------------------------- |
| A file, or several (a font's faces)                           | `MediaIdentity::of_file`, `of_files`: a metadata heuristic, below |
| Content a key always names (a hash, an immutable upload's id) | `MediaIdentity::of_key`                                           |
| Bytes handed over in memory                                   | `MediaIdentity::unique()`, taken again each time they are set     |

`of_file` doesn't read the file. It names it by its canonical path, length, modification time
and, on Unix, its device and inode, so replacing the file (as saving through a rename does) or a
write that changes its length or modification time gives a new identity. A write in place that
keeps both, such as one within the file system's timestamp resolution or with the time set back,
keeps the old identity, and renders keep the old bytes. A host that knows when its bytes change,
from immutable uploads, content hashes or version counters, should use `of_key` instead;
lumen-server names `asset:` sources by their asset id this way.

An image is decoded and uploaded once per identity and size. Nodes read a `media_in` raster's
texels as its pixels, so an image is reduced only where that can't change the picture: every
`media_in` that shows it must be read only by `resize` nodes with a literal `width`, `height` and
`mode`, in the root composition, and it is reduced only to sizes with its aspect ratio exactly
(a 4000×3000 photo to multiples of 4×3, a 1000×9 one not at all). Such an image is reduced to
fit the device's texture limit, or `GpuCompositionRenderer::set_max_image_side`, with a
`media.image_reduced` warning (for the first 32 sources per composition), and when the
composition shows it at under half its size it is kept at twice the size it is shown. For the
second, what follows each resize to the output may only scale by a literal `transform` scale or
keep pixels as they are (merges, colour and alpha nodes, crops, blurs, glows and shadows). Any
other use, such as a `transform` reading the image itself, a shader, a 3D scene, a template, or
a source picked by an expression, keeps the image at its own size: past
`set_max_image_side` it is shown unreduced, and past the device's limit it fails as
`media.too_large`. A reduced copy is averaged over each pixel's footprint in linear light on the
CPU, before upload. `ImageFileResolver` decodes a file at its full size first, so a file whose
decode needs more than the 512 MiB the decoder allows one image still fails as
`media.too_large` before any reduction.

The engine asks again on each frame that uses a source, to see whether its identity changed, so a
store should resolve an opened source cheaply and let font, model and environment resolvers read
their bytes only when asked. A source the store doesn't have fails as `media.not_found`, which for
a font family that isn't an asset reference makes its text fall back to other fonts. Typed access
goes through `media::resolve_as::<dyn ImageResolver>(store, source)`; a store that serves another
kind than asked fails there as `media.wrong_kind`, so a store that keeps one kind per name can
return whatever it has. A minimal store, from
[`examples/media_store.rs`](examples/media_store.rs):

```rust
impl MediaStore for MemoryStore {
    fn resolve(&self, source: &str, _kind: AssetKind) -> Result<ResolvedMedia, MediaError> {
        let images = self.images.read().unwrap();
        let (identity, frame) = images
            .get(source)
            .ok_or_else(|| MediaError::source_not_found(source, "no image was set"))?;
        Ok(ResolvedMedia::new(
            identity.clone(),
            MediaResolver::Image(Box::new(MemoryImage(Arc::clone(frame)))),
        ))
    }
}
```

## Caches and memory

A `GpuCompositionRenderer` keeps media and what it derives from them on the GPU between frames,
so later frames and renders skip decoding, uploading and drawing again. Each cache has a byte
budget, one field of `CacheBudgets`, set with `set_cache_budgets` and read with `cache_budgets`.
Past its budget a cache frees entries, least recently used first, until it is back within it or
only protected entries are left: what the latest frame uses, and for 3D caches what a kept output
was drawn from. Only `image_textures` counts just idle entries. The others count the cache's
total, protected entries included, so a large entry in use can free every idle one: a 100 MiB
table in use frees an idle 40 MiB one under the 128 MiB `luts` budget.

| What it holds                                                                                   | Field              | Counts                                  | Default |
| ----------------------------------------------------------------------------------------------- | ------------------ | --------------------------------------- | ------- |
| Image textures, by identity and decoded size                                                    | `image_textures`   | only those no frame binds               | 256 MiB |
| Pooled textures, retained outputs among them; spares are freed by size bucket, not strictly LRU | `pooled_textures`  | total; only spares are freed            | 384 MiB |
| Outputs of nodes that don't change with the frame (also dropped after 600 frames unread)        | `retained_outputs` | total; those the latest frame read stay | 1 GiB   |
| `.cube` lookup tables                                                                           | `luts`             | total; those the latest frame used stay | 128 MiB |
| Mip chains of sources shown smaller than their size (also dropped after 600 frames unused)      | `mip_chains`       | total; those the latest frame used stay | 256 MiB |
| 3D models: GPU buffers and textures, plus the CPU bounds kept beside them                       | `models`           | total; drawn or kept-output models stay | 256 MiB |
| 3D environment maps' textures                                                                   | `environments`     | total; as models                        | 256 MiB |
| Extruded `text_3d` meshes' GPU buffers (at most 256)                                            | `text_meshes`      | total; as models                        | 64 MiB  |

```rust
use lumen_engine::gpu::{CacheBudgets, GpuCompositionRenderer};

let mut renderer = GpuCompositionRenderer::new().await?;
renderer.set_cache_budgets(CacheBudgets {
    image_textures: 64 << 20,
    retained_outputs: 256 << 20,
    ..CacheBudgets::default()
});
let stats = renderer.cache_stats(); // bytes each cache holds now
```

`cache_stats` reports the bytes each cache holds, what frames use included, counted as its
budget counts them, and the 64 MiB glyph atlas once text is drawn. Its `pooled_textures` count
retained outputs too, as those are pooled textures. `release_resources` frees everything but the
atlas, and `release_text_atlas` that too.

Not every cache has a byte budget. The atlas, the bundled font faces, built-in 3D shapes and the
8 KiB environment lighting table (not in `environments`) have fixed sizes. Runtime programs and
shader sources are bounded by count, and samplers by the finite set of sampler settings. GPU video frame imports and bind groups are freed
only when unused for 600 frames or when what they bind goes, and uniform and storage arenas keep
the capacity of the largest frame. Audio keeps 64 mixed blocks and a resolver per source for the
mixer's lifetime. Text shaping keeps fonts until reset, and glyph extents, cleared once 16,384
are cached. A process-wide map shares font files while any text session holds them, with no
limit on how many, and one fallback text session lives for the process. Video decoders and their decoded frames, and decoded images on the CPU, belong to the
media store and its resolvers: `FfmpegVideoResolver` keeps up to 64 frames or 256 MiB per source.

## Audio

The root composition and each template can hold an `audio` timeline of tracks and clips. A
template's audio plays wherever a `clip` node that reaches the output plays the template, at the
template time the clip shows: its `start`, `duration`, `offset`, `speed` and `end` place the sound
as they place the frames, through nested clips. Speed resamples linearly, so pitch follows it. A
`hold` past the window, and a `time_remap` whose time holds or runs backwards, are silent. A
`switch` plays only its selected layer's audio. A template's solo tracks silence only its own
tracks.

A track's or clip's `volume` is a gain or an `=expression` of the time of the composition it is
in, such as `"=if(time >= 2 && time < 6, 0.25, 1)"` to duck music under a voice-over, or
`"=prop('level')"` in a template. Expressions can't read nodes, glyphs or text. Values below 0 are
silence. Clip `fade_in_ms` and `fade_out_ms` fade from and to silence at the clip's start and end
on an equal-power curve, `sin(π/2 · t)`, so a fade-out overlapping a fade-in keeps the loudness of
the two clips steady; on a clip shorter than both, the two multiply. Every clip also ramps over its
first and last 384 samples, and where a window cuts it, so it never clicks.

`audio::plan` evaluates gains and clip timing every 512 samples, with gains moving linearly in
between, into segments that `audio::AudioMixer` plays. The browser preview plays the same plan, so
both mix alike. A prop a clip sets by expression is evaluated at each of those points, in the time
of the composition the clip is in, so a gain that reads it moves smoothly too.

Planning is bounded. The root and each template may hold at most 512 audio tracks and 8192 audio
clips, and clips play templates with sound at most 4096 times in all, counting the clips inside the
templates they play; more fails `check` with `audio.invalid`. An interval looks up only the clips
that overlap it, so clips elsewhere on the timeline cost nothing. While planning, a 512-sample
interval may be inside at most 1024 clips and time remaps at once (each restart of a loop in the
interval counts, and a loop may restart at most 32 times in it) and may sound at most 1024 clips;
past either, planning fails with `audio.limit`. One call of `audio::plan` returns at most 500,000
segments (the server plans 30 s at a time, so about 178 clips with animated volumes sounding
throughout); past that it fails with `audio.limit`. The preview's plan of the whole timeline covers at
most about 3 hours (1,048,576 intervals) and holds at most 200,000 segments, which an animated
volume adds one of every 512 samples to; past either, loading the preview fails with
`audio.limit`.

## 3D depth and normals

The [3D guide](../../docs/3d.md) covers the scene nodes; this section covers what `render_3d` can output besides colour.

`render_3d`'s `output` picks what its raster holds: `color`, the lit scene (the default), `depth`
or `normal`. A node has one output, so a composition that wants colour and depth draws the same
scene with two `render_3d` nodes. The second shares what the first loaded and uploaded: models,
meshes, and the rasters image planes show, which are recorded once a frame. It adds a draw for
each batch of objects and one pass that stores what it drew, and skips lighting, shadows, the
environment and mipmaps.

- `depth` is the distance from the camera along its view (not from its eye, so a plane facing the
  camera is one value), mapped from 0 at `depth_near` to 1 at `depth_far` and clamped, as opaque
  grey. Either one at 0 uses the camera's `near` or `far`. Where nothing is drawn it is 1.
- `normal` is the normal each surface is lit by, after morph targets, skinning, flat shading and
  normal maps, and reversed on the back of a double-sided surface, in the camera's space: +X right,
  +Y up, +Z towards the camera. Each component `n` is stored as `n * 0.5 + 0.5`, opaque; where
  nothing is drawn it is transparent.

Both are data rather than colour: values go into the working space as they are, with no transfer
function, so a node reading the raster, such as `merge`'s luma mask or a `wgsl_shader`, reads the
value itself. An 8-bit output encodes them as it does light, so a depth of 0.5 is code 188.
Opaque surfaces, `text_3d` and instances included, draw into them everywhere, masked ones where
they reach their cutoff, and blended ones, such as image planes, where their alpha reaches half,
the same rule that decides where they block shadows. Shadow catchers draw too, since they hide
what is behind them. Each pixel holds the nearest surface at its centre: `supersample` is ignored,
so an edge is never averaged into a value no surface has.

Rasters hold half floats, so depth steps by 1/2048 between 0.5 and 1, and more finely below. Set
`depth_near` and `depth_far` around the part of the scene an effect needs, not to the camera's
default 0.05 to 1000.

Fog, with existing nodes, fades the scene into a colour by its depth:

```json
{ "id": 10, "type": "render_3d" },
{ "id": 11, "type": "render_3d", "params": { "output": "depth", "depth_near": 3, "depth_far": 14 } },
{ "id": 12, "type": "background", "params": { "paint": [205, 215, 228, 255] } },
{ "id": 13, "type": "merge", "params": { "mask_mode": "luma" } }
```

with the scene wired to both `render_3d` nodes, and `10` to `base`, `12` to `overlay` and `11` to
`mask` of the merge. A `levels` node between the depth and the mask shapes how the fog thickens.

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

Passing a `known_font` lookup also checks fonts: each family a `text` or `text_3d` node names as a literal
that the host can't resolve, other than the bundled `Roboto`, is a `font.fallback` warning. Renderers
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

### What a composition needs

`Composition::media_dependencies()` lists what a parsed composition needs from its host before it
renders, without rendering it: every distinct source its nodes and audio clips name as written (a
path, a URL, an alias or `asset:<id>`), the kinds of media each use reads, and where it is used,
in the root or in a template. A source an expression computes is listed under `dynamic` instead,
and the font families text nodes name as literals under `fonts`.

```rust
let composition = lumen_engine::json::parse(&json)?;
for dependency in &composition.media_dependencies().media {
    println!("{} ({:?}): {} uses", dependency.source, dependency.kinds, dependency.uses.len());
}
```

A node param is a media source when its param struct declares it with `#[media(image, video)]`,
`#[media(model)]` or `#[media(environment)]`; `by = kind` names the enum param whose options pick
one of the kinds. The declaration also gives the param's schema an `x-lumen-media` and the
TypeScript package its `nodeMediaSources` table. In the browser bindings it is
`mediaDependencies(json)`.

### What only rendering finds

`check` covers what can be told from the composition alone. These fail, or warn, only while
rendering:

- media that is missing or fails to decode;
- expressions that fail on some frames only, such as a division that is only invalid at frame 0;
- `wgsl_shader` code that doesn't compile;
- encoders that won't open;
- fonts that fall back at shaping time, including families set by expressions.

Passing the host's asset store also checks each `asset:` reference a source field holds as a
literal: one the store doesn't have is `media.not_found`, and one of another kind is
`media.wrong_kind`. Rendering raises the same errors.

### Media failures

Every way a media source can be unusable is a `MediaFailure`, whatever the kind (image, video,
audio, font, model or environment map) and whichever host renders:

```rust
match error.media_failure() {
    Some(failure) => match &failure.reason {
        MediaReason::NotFound { .. } => upload(&failure.source),
        MediaReason::WrongKind { .. } => println!("{} is {:?}", failure.source, failure.found),
        MediaReason::TooLarge { limit, actual, .. } => shrink(&failure.source, *limit, *actual),
        MediaReason::Unsupported { .. } | MediaReason::Decode { .. } => reject(failure),
    },
    None => {}
}
```

A failure has the `source` as the composition wrote it, the kinds the place using it `wanted`, the
kind it `found` only when that is established, where it is used (`at`: a node param, or an audio clip, in the root or
a template), and a `reason` with its public code. Diagnostics from `check` carry the JSON pointer
of that place.

| Code                | The source...                                                    |
| ------------------- | ---------------------------------------------------------------- |
| `media.not_found`   | isn't there                                                      |
| `media.wrong_kind`  | is there but is another kind than the use reads                  |
| `media.unsupported` | is the right kind, in a format or with a feature that isn't read |
| `media.too_large`   | is over a limit (`limit`, and `actual` when it is known)         |
| `media.decode`      | can't be read as what it should be                               |

`media.decode` and `media.unsupported` differ by what the engine knows: it says `unsupported` when
it identified the data (a container, a codec, a glTF extension, a font format, a `.gltf` JSON
document) and doesn't read it, and `decode` when the data is corrupt, truncated, contradicts itself,
or is in no format it can identify. A file that only starts like JSON is `decode`; a glTF JSON
document is recognised by a top-level `asset` object with a `version` within the first 64 KiB, and
is `unsupported`.

`found` is set only when it is established: by a format that holds one kind of media only (PNG,
glTF, a font, WAV), or by opening a container and finding which streams it has. A header can't tell
audio from video in MP4, Matroska/WebM or Ogg, so a file of one of these used where an image, model
or font is wanted is `media.wrong_kind` without a `found`; a container opened as video or audio has
`found` set from its streams. An unknown `found` is correct; a wrong one is not.

A failure that is a shortage which can pass (out of memory, file descriptors or a device in use, in
the engine's own reads and in FFmpeg's, while opening and while decoding) is a `media.decode` with
`transient` set, which hosts may retry.

### Codes

| Code                                 | Severity | Found by      | Meaning                                                                                                                                                                                                                                                                        |
| ------------------------------------ | -------- | ------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `document.syntax`                    | error    | check         | The text is not JSON.                                                                                                                                                                                                                                                          |
| `document.schema_version`            | error    | check         | `lumenSchemaVersion` is not a string, or names a version outside this engine's Lumen line.                                                                                                                                                                                     |
| `document.invalid`                   | error    | check         | A key or value outside node params has the wrong shape, is missing or is unknown.                                                                                                                                                                                              |
| `expression.function`                | error    | check         | A function in `functions` doesn't parse, or is defined twice or recursively.                                                                                                                                                                                                   |
| `node.unknown_type`                  | error    | check         | A node's `type` is not a node type.                                                                                                                                                                                                                                            |
| `node.invalid_params`                | error    | check         | A node's `params` is not an object.                                                                                                                                                                                                                                            |
| `node.unknown_param`                 | error    | check         | A node has a param its type doesn't.                                                                                                                                                                                                                                           |
| `node.invalid_param`                 | error    | check         | A param's value is wrong for it, including expressions that don't parse.                                                                                                                                                                                                       |
| `node.unsupported`                   | error    | check         | A node needs what this build of the engine was built without: any 3D node, without the `3d` feature.                                                                                                                                                                           |
| `graph.duplicate_node_id`            | error    | check         | Two nodes of one graph share an id.                                                                                                                                                                                                                                            |
| `connection.invalid_from_port`       | error    | check         | A connection's `from_port` is not `output`.                                                                                                                                                                                                                                    |
| `connection.missing_source_node`     | error    | check         | A connection or input reads a node that doesn't exist.                                                                                                                                                                                                                         |
| `connection.missing_target_node`     | error    | check         | A connection writes to a node that doesn't exist.                                                                                                                                                                                                                              |
| `connection.unknown_port`            | error    | check         | A connection writes to an input its node doesn't have.                                                                                                                                                                                                                         |
| `connection.input_already_connected` | error    | check         | A single input is connected twice.                                                                                                                                                                                                                                             |
| `connection.wrong_kind`              | error    | check         | A connection wires a scene into an input that takes a raster, or a raster into one that takes a scene.                                                                                                                                                                         |
| `graph.node_id_mismatch`             | error    | check         | A node is stored under another id. Only builders made in Rust can do this.                                                                                                                                                                                                     |
| `graph.missing_required_input`       | error    | check         | A required input is not connected.                                                                                                                                                                                                                                             |
| `graph.missing_media_output`         | error    | check         | A graph has no `media_output` node.                                                                                                                                                                                                                                            |
| `graph.multiple_media_outputs`       | error    | check         | A graph has more than one `media_output` node.                                                                                                                                                                                                                                 |
| `graph.cycle`                        | error    | check         | Nodes read each other in a cycle.                                                                                                                                                                                                                                              |
| `clip.invalid`                       | error    | check         | A clip names a composition that doesn't exist, sets props it doesn't declare or of the wrong kind, plays a composition and a `source` together, or has a negative `duration` other than -1.                                                                                    |
| `expression.invalid_prop_read`       | error    | check         | `prop()` is used outside a template, names an undeclared prop, or doesn't name it with a string literal.                                                                                                                                                                       |
| `settings.invalid`                   | error    | check         | A timeline, canvas or prop default can't render, such as an `fps` of 0.                                                                                                                                                                                                        |
| `audio.invalid`                      | error    | check         | Audio tracks or clips are duplicated, unknown, too many in one composition, or have invalid timing or volume, or a volume expression reads a node, a glyph, text or an undeclared prop.                                                                                        |
| `composition.cycle`                  | error    | check         | Templates instantiate themselves.                                                                                                                                                                                                                                              |
| `font.fallback`                      | warning  | check, render | A `text` or `text_3d` node's font family resolved to no font, so it renders in a fallback.                                                                                                                                                                                     |
| `font.no_outlines`                   | warning  | render        | A `text_3d` node's glyphs have no outlines to extrude, such as colour emoji, so they are left out.                                                                                                                                                                             |
| `scene.shadow_limit`                 | warning  | render        | A `render_3d` scene has more than 8 lights casting shadows, so those past the first 8 cast none.                                                                                                                                                                               |
| `scene.model_primitives_skipped`     | warning  | render        | A `model_3d`'s model has points, lines, or triangle strips or fans, which aren't drawn.                                                                                                                                                                                        |
| `media.color_unsupported`            | warning  | render        | A video's colour tags name a transfer or primaries the engine doesn't convert, so it renders as SDR or BT.709 in their place, or an image's ICC profile can't be honoured, so it renders as sRGB.                                                                              |
| `media.image_reduced`                | warning  | render        | An image every use of which is a literal `resize` is larger on a side than the device's texture limit, or the host's `set_max_image_side`, so it is shown from a copy reduced to fit.                                                                                          |
| `property.invalid_type`              | error    | render        | An expression gave a param a value of the wrong type.                                                                                                                                                                                                                          |
| `expression.parse`                   | error    | render        | An expression built at render time doesn't parse.                                                                                                                                                                                                                              |
| `expression.evaluate`                | error    | render        | An expression failed to evaluate.                                                                                                                                                                                                                                              |
| `media.not_found`                    | error    | check, render | A media source isn't there: a path or alias the host doesn't have, or an `asset:` id its asset store doesn't have.                                                                                                                                                             |
| `media.wrong_kind`                   | error    | check, render | A media source is another kind than the place using it reads, such as an `asset:` image used as a model, or a video file given where an image is read.                                                                                                                         |
| `media.unsupported`                  | error    | render        | A media source is the right kind but in a format or with a feature the engine doesn't read: an unknown container or codec, BT.2020 constant-luminance video, a glTF using a required extension, a `.gltf` JSON file, a WOFF font.                                              |
| `media.too_large`                    | error    | render        | A media source is over a limit, in bytes, pixels or counts, or is bigger than the device can hold. The limit and, when known, the actual value are in the failure.                                                                                                             |
| `media.decode`                       | error    | render        | A media source is meant to be what the place reads but can't be read: it is corrupt, truncated or in no format the engine can identify.                                                                                                                                        |
| `media.frame_out_of_range`           | error    | render        | A video frame past the end of its source was asked for.                                                                                                                                                                                                                        |
| `audio.unknown_track`                | error    | render        | An audio clip names a track that doesn't exist.                                                                                                                                                                                                                                |
| `audio.limit`                        | error    | render        | Audio is more than the plan can hold: too many clips playing sound at once, a loop restarting too often, too many segments in one plan, or a preview timeline too long.                                                                                                        |
| `render.frame_out_of_range`          | error    | render        | A frame past the composition's end was asked for.                                                                                                                                                                                                                              |
| `render.node_evaluation`             | error    | render        | A node failed to render, such as text with too many glyphs or a `wgsl_shader` that doesn't compile.                                                                                                                                                                            |
| `render.missing_node`                | error    | render        | A node read while rendering doesn't exist.                                                                                                                                                                                                                                     |
| `render.invalid_output_type`         | error    | render        | A node's output had the wrong type for its reader.                                                                                                                                                                                                                             |
| `render.gpu`                         | error    | render        | The GPU failed: the device, its queue or a driver. Nothing the composition says causes it, so trying again may succeed.                                                                                                                                                        |
| `render.limit`                       | error    | render        | The composition needs more than the device or engine allows, such as a frame whose data is too large for the device's buffers or a video frame larger than its texture limit.                                                                                                  |
| `render.internal`                    | error    | render        | The engine, or a host's resolver or render target, broke a contract the render relies on, such as a poisoned lock or a stale recorded frame uploaded. Not the composition's fault. Trying again helps only for fonts that kept loading, which stay loaded for the next render. |
| `render.too_deeply_nested`           | error    | render        | A node is more than 48 nodes deep, counting through clips. Flatten the chain: put many layers in one `raster_multimerge` instead of chaining `merge`s, and combine consecutive transforms or adjustments.                                                                      |

## Features

- `ffmpeg`: enable FFmpeg-backed media handling through `lumen-engine-ffmpeg`.
- `image`: enable image decoding helpers.
- `metal`: enable Metal support through `lumen-engine-gpu` and optional FFmpeg interop.
- `vulkan`: enable Vulkan support through `lumen-engine-gpu` and optional FFmpeg interop.
- `cuda`: enable CUDA support through optional FFmpeg interop.
- `3d`: render the 3D scene nodes through `lumen-engine-3d`. Without it, a composition with any of
  them fails `check` with `node.unsupported`.

## Development

```bash
cargo check -p lumen-engine
cargo test -p lumen-engine
cargo check -p lumen-engine --features ffmpeg,image
```
