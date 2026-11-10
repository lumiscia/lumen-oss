---
"@lumiscia/lumen-bindings": patch
---

Color grade, crop and the composition output read a small layer at its own size instead of first copying it into a full-canvas texture, which saves a full-frame pass per use.
