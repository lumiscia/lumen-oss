---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-shared": minor
"@lumiscia/lumen-types": minor
---

Composition errors now name the JSON path, such as `timeline.fps: ...`, and more mistakes fail instead of being ignored. Unknown keys in `timeline` and `render_settings` are errors, including in templates, so a typo like `fpss` no longer falls back to 30 fps. Other unknown keys at the root and in `audio` are still ignored.

`render_settings.background_color`, which rendering never read, is an error: connect a `background` node instead. `Composition.setBackgroundColor` is gone. Audio clips need whole-number `start_ms` and `duration_ms`, and `source_start_ms` must be a whole number when set; `start_frame`, `duration_frames` and `source_start_seconds` are errors. `schemaVersion` is ignored and the schema no longer types it, `lumenSchemaVersion` must be absent or `0.1.0` in the schema too, and a non-string `$schema` or a `metadata` that isn't an object with a string `name` now fails at load, as it already did in the schema. The builder no longer writes `durationSeconds` into `timeline`. `CompositionBuilder::build` now rejects fps that isn't positive and finite, a zero size or duration, and a prop default of the wrong kind, for the root and templates.
