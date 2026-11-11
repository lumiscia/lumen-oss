---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-types": minor
"@lumiscia/lumen-preview": minor
---

Every media source that can't be used fails the same way, for images, videos, audio, fonts, models and environment maps, in the engine, `check`, `lumen-server`, `lumen-local` and the preview.

A failure names the source as the composition wrote it, the kinds the place using it reads, the kind it is when that is another one, where it is used (node and param, audio clip, template), and why, with one of four public codes:

| Code                | Means                                                                                                                                       |
| ------------------- | ------------------------------------------------------------------------------------------------------------------------------------------- |
| `media.not_found`   | Nothing is at the source: no file, no URL answer, no asset with that id                                                                     |
| `media.wrong_kind`  | The source is another kind of media than the use reads, such as a video in an `image` node or a font that is a PNG                          |
| `media.unsupported` | The engine identifies the data (a container, codec, glTF extension, `.gltf` JSON, WOFF) and doesn't read it                                 |
| `media.too_large`   | Over a limit (file bytes, pixels, the device's texture size, a model's counts), with the `limit` and the `actual` value when they are known |
| `media.decode`      | The data is corrupt, truncated, contradicts itself, or is in no format the engine identifies                                                |

`media.decode` and `media.frame_out_of_range` keep their codes. `check` reports missing and wrong-kind `asset:` references with these codes and the JSON pointer of the param or audio clip, also inside templates. `lumen-server` carries the engine's code through unchanged, and marks a render retryable only for a shortage that can pass (an `EMFILE`, out of memory or similar I/O failure). The upload probe's rejection reasons end with the code, such as `(media.too_large)`. `lumen-local` prints `lumen-local failed (media.not_found): ...`.

The thrown error of the WASM bindings carries `code` (the dotted code for a media failure) and `media` (the structured fields). `LumenPreviewError` has a new `media` field with them, typed as `MediaFailure` in `@lumiscia/lumen-types`, and it is `null` for other failures. A preview that is given an environment map now fails with `media.unsupported`.

Migrating:

| Before                                                                         | After                                                                                           |
| ------------------------------------------------------------------------------ | ----------------------------------------------------------------------------------------------- |
| `asset.not_found` (check, render)                                              | `media.not_found`, with the source `asset:<id>`                                                 |
| `asset.wrong_kind` (check, render)                                             | `media.wrong_kind`                                                                              |
| `media.not_found` for a missing file, URL, font or model                       | `media.not_found` (unchanged), now with the kinds wanted and where it is used                   |
| `media.decode` for a file of another kind, an unsupported codec or a size cap  | `media.wrong_kind`, `media.unsupported` or `media.too_large`; corrupt data stays `media.decode` |
| Rust `MediaError::{SourceNotFound, Decode, AssetNotFound, AssetWrongKind}`     | `MediaError::Failed(MediaFailure)`; build one with `MediaError::source_not_found` or `decode`   |
| The 3D crate's `ModelError::TooLarge(String)` and `EnvironmentError::TooLarge` | `TooLarge { details, limit, actual }`                                                           |
| Messages such as ``failed to decode image `a.png`: ...``                       | ``image `a.png` (node 3 `source`) failed to decode: ...``; match the `code`, not the message    |
| An upload probe's rejection reason                                             | The same reason followed by its code, such as `... (media.decode)`                              |

```json
{
  "severity": "error",
  "code": "media.wrong_kind",
  "path": "/nodes/4/params/source",
  "node": 5,
  "message": "`asset:clip` (node 5 `source`) is a video, not an image"
}
```
