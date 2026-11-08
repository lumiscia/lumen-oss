---
"@lumiscia/lumen-sdk": minor
"@lumiscia/lumen-bindings": minor
---

`POST /renders` on `lumen-server` accepts `quality: { bitrate, crf, qp, preset }`, with the same meanings and rules as `lumen-local`'s flags: at most one of `bitrate` (a number or text such as `"14M"`), `crf` and `qp`. Invalid combinations or values return `422`. `RenderOptions.quality` in the SDK sends it. For Rust users of `lumen-engine-ffmpeg`, `EncoderQuality::from_options` and `parse_bitrate` hold the checks `lumen-local` and `lumen-server` share.
