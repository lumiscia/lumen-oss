---
"@lumiscia/lumen-bindings": minor
---

Gradients now reject what they used to ignore. A gradient with more than eight stops fails to load instead of silently dropping the highest offsets, and the schema caps `stops` at eight. A gradient with an unknown key, such as a misspelt `spred`, is an error instead of being skipped. In Rust, `Paint::to_gpu` no longer takes a fallback colour: a gradient without stops draws transparent.
