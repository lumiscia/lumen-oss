---
"@lumiscia/lumen-bindings": patch
---

`raster_multimerge` composites up to eight overlapping layers in one pass instead of one pass per layer, so a stack of large layers costs about a third of the GPU time it did.
