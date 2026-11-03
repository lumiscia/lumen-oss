---
"@lumiscia/lumen-bindings": minor
---

Measuring text and then drawing it now shapes it once: one cache of the 32 most recent shapes replaces the separate fit and measurement caches. Solid `background` paints no longer run their supersampling loop, and the text shader only reads the atlas in the branch that uses it. Rendered output and composition JSON are unchanged. For Rust: `TextSystem::shape` now returns `Arc<ShapedText>`, and `TextSystem::shape_changed` shapes text without keeping it.
