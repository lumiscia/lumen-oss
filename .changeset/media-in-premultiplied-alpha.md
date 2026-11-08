---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-types": minor
---

`media_in` takes an `alpha` param for video: `"straight"` (the default, as before) or `"premultiplied"`, for sources like many ProRes exports whose colour is already multiplied by alpha and would otherwise be premultiplied twice and show dark edges. For Rust users, `MediaFrame` has a new `RawRgba` variant for RGBA frames whose premultiplication is left to the node reading them; FFmpeg RGB video with alpha now comes out as one.
