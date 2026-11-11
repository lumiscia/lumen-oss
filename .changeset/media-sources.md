---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-types": minor
"@lumiscia/lumen-preview": minor
---

A node param is declared as a media source in one place, and a composition can list what it needs before it renders.

A param struct marks a `String` param with `#[media(image, video)]`, `#[media(model)]` or `#[media(environment)]`, and `by = kind` names the enum param that picks among several kinds. `media_in.source`, `model_3d.source`, `render_3d.environment_map` and an audio clip's `source_id` are declared this way. Asset validation, the schema and the browser preview now read the declaration instead of each knowing which nodes take sources.

`Composition::media_dependencies()` returns the distinct sources a composition's nodes and audio clips name as written, with the kinds each use reads and where it is used (node and param, or audio clip, inside a template too). A source an expression computes is listed as dynamic, and the font families text uses are listed apart from media. The browser bindings expose it as `mediaDependencies(compositionJson)`, typed as `MediaInventory` in `@lumiscia/lumen-types`.

The generated schema gives each such param `x-lumen-media` (`{ "kinds": ["image", "video"], "selectedBy": "kind" }`), and `@lumiscia/lumen-types` exports the generated `nodeMediaSources` table (node kind → param → kinds) with `MediaSourceParam` and `MediaSourceKind`. The preview reads it to tell what a `media` entry holds.

**Breaking for Rust:** `PropertyDef` has a `media` field, and the `Node` and `NodeParams` traits have a required `for_each_media_source`; hand-written implementations must add it (the `Node` and `Delegate` derives do). `lumen_engine::media::resolve_asset_of` accepts any of several kinds. `LumenPreviewBindings` has a required `mediaDependencies`, so a bindings object built by hand must add it. Nothing changes for compositions.

A name in `compositions` may be at most 256 bytes long; a longer one fails with `document.invalid`.
