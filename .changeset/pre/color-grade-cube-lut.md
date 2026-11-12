---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-types": minor
"@lumiscia/lumen-preview": minor
---

**Breaking: `color_grade` has one LUT scale, and its inline LUT is the `lut` param.** `color_grade` also reads 3D `.cube` LUTs from files with the new `lut_source` param.

- The inline 1D LUT, which was `lut_source`, is now `lut` (`"rgb1d:0,0,0;0.2,0.1,0.3;1,1,1"` or `"identity"`). Rename the param in existing compositions.
- Its scale is 0 to 1. The engine no longer guesses per value whether a number above 1 was meant as 0–255. A value outside 0..1 or not finite fails the render, naming `lut` and the value. **Migration:** divide a 0–255 LUT by 255 (`rgb1d: 0,0,0; 255,128,0` becomes `rgb1d: 0,0,0; 1,0.5,0`).
- `lut_source` is now a media source, a path, `media` alias or `asset:<id>`, naming an Adobe/Resolve `.cube` file. It reads `TITLE`, `LUT_3D_SIZE`, `DOMAIN_MIN` and `DOMAIN_MAX`, comments and blank lines, and uploads the table as a 3D texture once per media identity. Lookups are trilinear, with the half-texel offset so the end points land on the first and last entries; `interpolation: "nearest"` takes the nearest entry. `strength` blends the graded colour as it does for `lut`. Table values outside 0..1 are kept.
- The lookup is in the sRGB-encoded domain on straight colour: the colour is unpremultiplied, encoded, clamped to 0..1, looked up, decoded and premultiplied again, and alpha is untouched.
- A file is read as untrusted input: `LUT_3D_SIZE` up to 129, files up to 64 MiB, and a missing or wrong entry count, a non-finite or out-of-range number, a missing size line, and any other keyword (`LUT_1D_SIZE`, `LUT_3D_INPUT_RANGE`, ...) are errors that say which. 1D `.cube` tables are not supported.
- Setting both `lut` and `lut_source` is an error. A literal `lut` that is out of range or malformed, both params set, and an inline LUT written in `lut_source` (as old compositions had it) are `node.invalid_param` diagnostics at the param when the composition is checked; an expression-valued param fails at render with the same message.

**Rust API:** `AssetKind::Lut`, `Asset::Lut`, `MediaResolver::Lut`, the `LutResolver` trait and `MAX_LUT_BYTES` are new, and `#[media(lut)]` declares a param of that kind. A `MediaStore` that matches on `AssetKind` handles the new variant (a store with no LUTs answers `media.not_found`). `lumen_gpu::LayoutKind::Texture3d` binds a filterable 3D texture.
