---
"@lumiscia/lumen-bindings": minor
---

Node compilers in `lumen-engine` now return a `RasterHandle` directly, and rasters carry `alpha_mode` rather than a `RasterMetadata`. Removed from the Rust API: `CompiledOutput`, `RasterMetadata`, `ColorSpace`, `CompiledComposition::node_outputs`, `CompositionMetadata` and `Composition::metadata`, `TimelineSettings::time_seconds`, `BoundFrame::write_texture_rgba8` and `write_texture_rgba16_float`, `error::Warning`, and the error variants nothing raised. Compositions render the same, and a `metadata` key in composition JSON is still accepted and ignored.
