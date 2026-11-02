---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-shared": minor
---

Compositions now fail to load instead of silently rendering something else. These are errors: `params` that isn't an object, unknown keys on nodes and connections (such as `properties` for `params`), duplicate node ids within a graph, repeated audio track or clip ids, audio clips on a track the timeline doesn't declare, and non-finite or negative audio gains. The audio checks also apply to compositions built in Rust. `lumen-server` and WASM parse errors now include the nested cause, such as the bad param inside a template, not just `composition `card``. `AudioTrack.addClip` now gives clips without an id the default `<track id>-clip-<n>`, so clips on different tracks don't collide.

Media now renders correctly. A canvas-sized `media_in` wired straight to `media_output` writes to external targets, where GPU encodes previously got an empty frame. Media that changes size renders at its own size instead of stretched into the old one. Replacing an image in a `LumenMediaStore` with one of the same size shows the new pixels.

For Rust: `MediaTextureKey` loses `frame`, `MediaTextureUpload::key` and `BoundFrame::use_media_texture` take `Option<MediaTextureKey>`, with `None` for video frames, and `GraphValidationError::InvalidAudio` and `MediaError::UnknownAudioTrack` are new.
