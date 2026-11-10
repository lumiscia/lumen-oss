---
"@lumiscia/lumen-bindings": patch
---

Videos cut with `ffmpeg -c copy` no longer count their hidden preroll frames, so renders near the end of the clip work.
