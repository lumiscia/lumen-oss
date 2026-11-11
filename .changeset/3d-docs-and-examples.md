---
"@lumiscia/lumen-bindings": patch
"@lumiscia/lumen-preview": patch
"@lumiscia/lumen-sdk": patch
"@lumiscia/lumen-shared": patch
"@lumiscia/lumen-types": patch
---

Documentation for 3D. The `lumen-sdk`, `lumen-shared` and `lumen-preview` READMEs link to the new 3D guide, and `lumen-shared`'s shows a scene. The descriptions of a 3D material's `color` and `shadow_catcher` params, which `lumen-types` carries as doc comments and `lumen-bindings` as node metadata, now say that only an image plane has a raster to multiply by `color` or to count in `shadow_catcher`. Nothing about how a composition renders changes.
