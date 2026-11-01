---
"@lumiscia/lumen-bindings": minor
---

The Rust `lumen-engine::Composition` is now read-only and fingerprints its content once when built. Construct it with `CompositionBuilder::new(graph, timeline, render_settings).build()`; structural validation now happens during `build()`, which returns `Result<Composition, Vec<LumenError>>`. Read its parts with `graph()`, `timeline()`, `render_settings()`, `metadata()`, `audio()` and `compositions()`. To edit, consume it with `into_builder()`, change the builder's public fields and call `build()` again. This replaces direct field access and `Composition::new`. TypeScript composition builders and the JSON format are unchanged.
