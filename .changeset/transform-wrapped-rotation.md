---
"@lumiscia/lumen-bindings": patch
---

A `transform` rotated by many whole turns renders the same as its rotation within one turn on every GPU. Metal's fast sine and cosine returned 0 for angles past about ten million radians, so such a transform drew nothing; its angle is now wrapped to a turn before the shader sees it, which also lets these transforms keep tight bounds.
