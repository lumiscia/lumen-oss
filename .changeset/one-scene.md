---
"@lumiscia/lumen-bindings": minor
---

For Rust: `CompositionBuilder` and `CompositionTemplate` hold their graph, timeline and render settings in a new `Scene`, as `scene`, and `Scope` holds `scene: &Scene`. `Composition::graph`, `timeline` and `render_settings` still work, and `Composition::scene` is new. Composition JSON and rendering don't change.
