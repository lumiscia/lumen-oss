---
"@lumiscia/lumen-bindings": minor
---

Media that can't be opened now says why. A corrupt image used to report `media source not found`, and `lumen-local` printed the real cause separately; the render error now carries it, as in `media decoding failed for hero.png: failed decoding image source: ...`. `lumen-local` also names the path it looked at for a missing file. `lumen-server` reports `media source `hero.png` not found: no such file in the media root` and doesn't say whether a file exists outside the root.

For Rust: `MediaStore::get_image_resolver`, `get_video_resolver` and `get_audio_resolver` return `Result<Box<dyn _>, MediaError>` rather than `Option`, and default to `SourceNotFound`, so a store only implements the kinds it serves. `MediaError::SourceNotFound` gains `details`, and a file with no audio stream now fails with `MediaError::Decode`.
