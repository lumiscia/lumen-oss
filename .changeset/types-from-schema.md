---
"@lumiscia/lumen-types": minor
"@lumiscia/lumen-bindings": minor
---

`@lumiscia/lumen-types` now generates every node and composition type from `composition.schema.json`, so node params carry their schema descriptions as JSDoc and the root types can no longer drift from the schema. Enums that several nodes share are one type (`BlendMode`, `LoopMode`, `MaskMode`, `ChannelSelector`, `MediaInSourceKind`) and the old per-node names are aliases of it. New exports are `AudioClip`, `AudioTrack`, `RenderSettings` and `Timeline`. `MetaManifest` is gone, and `Connection`, `CompositionTemplate` and `LumenComposition` are type aliases rather than interfaces. `timeline` and `render_settings` now accept extra keys, as the schema and engine always did.

The composition schema defines `timeline` and `render_settings` once in `$defs` and gives `metadata`, `$schema`, `lumenSchemaVersion` and `schemaVersion` root types: a non-string version or `$schema`, or a `metadata` that is not an object with a string `name`, is now rejected. The `fps` default of 30 moves from the schema to its description. Consumers that vendor the schema need the new copy.
