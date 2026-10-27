---
"@lumiscia/lumen-bindings": patch
---

Shapes, paths, solid-colour text and transformed layers render into textures sized to the pixels they cover rather than the whole canvas, and merges, multimerges, opacity and colour filters work within those bounds, cutting GPU memory and frame time for compositions with many small elements. Output frames are unchanged.
