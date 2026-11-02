---
"@lumiscia/lumen-bindings": patch
---

Each `lumen-server` job and each WASM media store now keeps its own fonts, so two jobs can use different files under one family name, one after the other or at once. Before, the first file loaded for a name was used by every later job in the process.

A font that reaches a job now replaces any face already under that family name, including the bundled Roboto. A font file with no readable face fails the render, naming the family, instead of silently drawing fallback glyphs. A file whose faces go by another name logs a warning. `text_width` and `text_height` default to Roboto rather than whichever font was loaded first. `lumen-server` looks for a missing font once per job rather than on every frame.

For Rust: `MediaStore::text_session` returns the store's new `TextSession`. A store that serves its own fonts should return one, as without it fonts load into a session shared across the process. `font_resolver_id` returning `None` now means the store has no such font, and resolvers with one ID must serve the same data from any store.
