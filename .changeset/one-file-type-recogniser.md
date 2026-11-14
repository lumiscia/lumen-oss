---
"@lumiscia/lumen-bindings": patch
---

The engine now recognises BMP and ICO files from their first bytes, so a BMP given where a video is wanted is reported as "an image" instead of unreadable. `lumen-server`'s upload check decides what a file is with the same recogniser as renders, so a format added to one is no longer missing from the other. Uploads are accepted and rejected as before.
