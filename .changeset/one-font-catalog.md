---
"@lumiscia/lumen-bindings": minor
---

`lumen-server` and `lumen-local` now find fonts the same way: by the family name inside each font file, in `LUMEN_FONT_DIR`, then `<media-root>/assets/fonts`, then the system fonts. `lumen-server` used to guess file names under `LUMEN_MEDIA_ROOT/fonts`, so it never read the system fonts and ignored `LUMEN_FONT_DIR`. `LUMEN_MEDIA_ROOT` no longer supplies fonts, and a font file is no longer found by its file name or path. A `--font-dir` or `LUMEN_FONT_DIR` that isn't a directory is an error at startup instead of silently using other fonts. A family that no font has now logs a warning once and renders in the bundled font, as before. An installed Roboto no longer replaces the bundled one, and the demo compositions drop their `font_family`.

For Rust: `FontCatalog` moves from `lumen-local` to `lumen_server::render`, with `existing_font_dir` and `font_dir_from_env`.
