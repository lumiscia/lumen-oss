---
"@lumiscia/lumen-bindings": minor
---

For Rust users of `lumen-engine-text`: `TextLayoutRequest` now describes shaping only. `origin`, `color` and `disable_hinting` are gone, along with `TextSystem::layout` and `TextGlyph::color`; use `system.shape(&request).place(origin)`. `prepare_stroked_glyphs` is now `prepare_glyphs`, `PreparedGlyphs::frame` and `compacted`, `TextLayout::measurement`, and `GlyphAtlas::len` and `is_empty` are removed, and `TextAlign::Justified` is now `Justify`. Rendering and composition JSON are unchanged.
