---
"@lumiscia/lumen-bindings": minor
---

**Breaking (Rust API): a host supplies every kind of media through one `MediaStore::resolve`, and every resolved source carries a `MediaIdentity`.** Compositions, renders and the browser bindings' JavaScript API are unchanged; this breaks every Rust embedder that implements or calls `MediaStore`.

`MediaStore` has one required method, `resolve(&self, source: &str, kind: AssetKind) -> Result<ResolvedMedia, MediaError>`, plus `text_session` as before. `ResolvedMedia { identity, resolver }` holds a `MediaResolver`, an enum with one variant per kind wrapping that kind's existing resolver trait object. `media::resolve_as::<dyn ImageResolver>(store, source)` resolves and returns `(MediaIdentity, Box<dyn ImageResolver>)`; a store that serves another kind than asked fails there as `media.wrong_kind` with `wanted` and `found` set, so a store that holds one kind per name can return whatever it has.

`MediaIdentity` replaces each resolver's `id()`, with one rule: equal identities, from any store in the process, hold the same bytes, and a host gives a source a new one when its bytes change. Every cache that outlives a frame keys on it: image textures, retained outputs drawn from images and text, loaded fonts, GPU model buffers and environment maps. Build one with `MediaIdentity::of_file(path)` or `of_files(paths)` (path, length and modification time), `of_key(key)` for a content hash or an immutable upload's id, or `unique()` for bytes handed over in memory, taken again on each set. `ImageFileResolver` and `FfmpegVideoResolver` named themselves by their path, so a renderer kept across stores could draw a replaced image's old pixels; lumen-server now names files by path, length and modification time for every kind, as it already did for models, environment maps and fonts.

A font family the store doesn't have is now `Err(media.not_found)` rather than `Ok(None)`; the engine falls back to other fonts for it unless the family is an `asset:` reference, which fails as before. Video decode options stay on the store, as `LocalMediaStore` already kept them.

| Before                                                                                                            | After                                                                               |
| ----------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------- |
| `fn get_image_resolver(&self, source)`                                                                            | `fn resolve(&self, source, AssetKind::Image)` returning `MediaResolver::Image`      |
| `fn get_video_resolver(&self, stream_id)`                                                                         | `resolve(source, AssetKind::Video)` returning `MediaResolver::Video`                |
| `fn get_audio_resolver(&self, source)`                                                                            | `resolve(source, AssetKind::Audio)` returning `MediaResolver::Audio`                |
| `fn get_model_resolver(&self, source)`                                                                            | `resolve(source, AssetKind::Model)` returning `MediaResolver::Model`                |
| `fn get_environment_resolver(&self, source)`                                                                      | `resolve(source, AssetKind::Environment)` returning `MediaResolver::Environment`    |
| `fn get_font_resolver(&self, family) -> Ok(Some(..))`                                                             | `resolve(family, AssetKind::Font)` returning `MediaResolver::Font`                  |
| `fn get_font_resolver(&self, family) -> Ok(None)`                                                                 | `Err(MediaError::source_not_found(family, ..))`                                     |
| `fn font_resolver_id(&self, family)`                                                                              | `resolve(family, AssetKind::Font)?.identity`, so fonts need their bytes read lazily |
| `ImageResolver::id`, `VideoFrameResolver::id`, `FontResolver::id`, `ModelResolver::id`, `EnvironmentResolver::id` | removed; return the identity in `ResolvedMedia`                                     |
| calling `store.get_image_resolver(source)`                                                                        | `media::resolve_as::<dyn ImageResolver>(store, source)`, likewise for each kind     |
| a server or file id string such as `format!("{path}:{len}:{mtime:?}")`                                            | `MediaIdentity::of_file(path)`                                                      |
| a version counter per registration                                                                                | `MediaIdentity::unique()` on each set                                               |
| `MediaTextureKey::source`                                                                                         | `MediaTextureKey::identity`                                                         |
