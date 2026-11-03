---
"@lumiscia/lumen-bindings": minor
---

For Rust: `text` params group into `layout`, `decorations`, `draw` and `glyph`, spans into `layout` and `style`, and evaluated `TextContent` is always a list of spans holding what shapes the text, a string being one span. Composition JSON and the schema stay flat, and rendering does not change.
