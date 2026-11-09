---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-types": minor
---

Source fields accept asset references, `asset:<id>`, naming a file in the host's asset store: `media_in`'s `source`, an audio clip's `source_id`, and a text node's or span's `font_family`. The `asset:` prefix is a scheme, so it can't be mistaken for a relative path, and plain paths and the `media` manifest keep working as before. An id the host doesn't have is an `asset.not_found` error, and an asset of another kind than its field reads, such as a font used as an image, is `asset.wrong_kind`. A host gives the engine its store as an `AssetResolver`, which resolves an id to an `Asset` (an image, video or audio file, or a font family's face files), and passing it to `json::check` checks literal references before rendering. `lumen-local` and `lumen-server` have no asset store yet, so references are `asset.not_found` errors on both.

A font asset is one family, and weight and style choose among its faces. Fonts now shape under the name the composition asks for: a font staged under a `media` alias, or any file whose faces carry another family name, used to load and then silently fall back, and now draws as asked. Its faces answer to that name instead of their own, so they can't stand in for another font with the same internal family name. For Rust users, `MediaStore::get_font_resolver` returns a `Result`, and `json::check` and `check_value` take the asset store as a third argument.
