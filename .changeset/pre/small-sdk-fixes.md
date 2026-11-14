---
"@lumiscia/lumen-shared": patch
"@lumiscia/lumen-sdk": patch
---

`fn()` now rejects a signature the engine would reject, such as `1bad(x)` or `f(x + y)`, when you call it instead of when the composition renders. The function name and each parameter must be an identifier: a letter or `_`, then letters, digits or `_`.

`uploadMedia` no longer throws when it cannot tell a file's content type. It sends `application/octet-stream`, which hosts accept because they read the file's bytes to tell what it is. It also names `.ttf`, `.otf`, `.woff`, `.woff2`, `.hdr`, `.exr`, `.cube` and `.gltf` files by their extension.

`RenderOptions.idempotencyKey` is documented as honoured only by the hosted API; `lumen-server` ignores it.
