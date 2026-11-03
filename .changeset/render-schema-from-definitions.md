---
"@lumiscia/lumen-bindings": patch
---

`lumen-generators` renders the composition schema straight from the node definitions instead of through a second copy of their metadata. The generated schema is unchanged. The binary now takes the output directory as its only argument, and the crate exposes `generate` and `render_composition_schema_json` in place of its argument parser and manifest types.
