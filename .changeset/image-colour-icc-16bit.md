---
"@lumiscia/lumen-bindings": minor
---

Images follow their embedded ICC profile, and 16-bit images keep their depth. Until now every image was read as 8-bit sRGB, so a Display P3 photo (as iPhones take) looked dull, an Adobe RGB one washed out, and a 16-bit PNG or TIFF was cut to 8 bits before the float pipeline. Nothing changes in compositions: a tagged or deep image just renders right.

- **Profiles:** the embedded profile of PNG (`iCCP`), JPEG (`APP2`), WebP, TIFF and GIF is read with `moxcms`. Matrix/TRC RGB profiles (sRGB, Display P3, Adobe RGB, ProPhoto, Rec. 2020 and the like) and grey profiles convert to the working space's linear BT.709, colour outside BT.709 clipped at zero and kept above 1, as video is. An image without a profile is sRGB, as before; PNG `sRGB`, `gAMA` and `cHRM` chunks are not read.
- **sRGB stays exact:** an 8-bit image with no profile, or one whose profile matches sRGB, decodes and uploads exactly as before, so it still passes through bit for bit.
- **Wide gamut on the GPU:** an 8-bit image whose profile has the sRGB curve in other primaries, such as Display P3, keeps its 8-bit texture and converts in linear light on the GPU, at no extra memory or time.
- **Deeper images:** 16-bit PNG and TIFF, float TIFF (read as encoded values, as before), and 8-bit images with another curve, such as Adobe RGB, are converted on the CPU to linear half floats, 8 bytes a pixel in the image texture cache (`image_textures` counts them so). A reduced copy is filtered at that precision.
- **Profiles that can't be honoured** (lookup-table, CMYK, a grey profile on a colour image or the reverse, past 1 MiB, or malformed) read as sRGB, and the render warns once per composition, `media_in` node and source with `media.color_unsupported`, the code untagged video colour already uses, naming why. A profile never fails a render.

**Breaking, for Rust hosts:**

- `MediaFrame` has two more variants, `WideRgba { frame, gamut }` and `LinearRgba(Arc<LinearMediaFrame>)`, which `ImageFileResolver` returns for wide-gamut and deep images. A `match` on `MediaFrame` needs arms for them.
- `Warning::UnsupportedVideoColor { video, .. }` is now `Warning::UnsupportedColor { kind, media, .. }`, with `kind` `AssetKind::Video` or `AssetKind::Image`. Its code and message for video are unchanged.
- `ImageResolver` has `color_warning()`, `None` by default. A resolver that wraps another should forward it, or the wrapped image's warning is lost.
