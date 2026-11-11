---
"@lumiscia/lumen-bindings": minor
---

Images are decoded no larger than they are shown.

Nodes read an image's texels as its pixels, so an image is reduced only where every use of it is a `resize` with a literal `width`, `height` and `mode`, in the root composition, and only to a size with its aspect ratio exactly, so the resize draws the same picture. Such an image, larger on a side than the device's texture limit, now renders from a reduced copy with a `media.image_reduced` warning (for the first 32 sources per composition) instead of failing as `media.too_large`. `GpuCompositionRenderer::set_max_image_side` sets a lower limit of the host's own. An image used any other way keeps its own size: past `set_max_image_side` it is shown unreduced, and past the device's limit it still fails as `media.too_large`.

When those resizes show the image at under half its size, it is decoded at twice the size it is shown: a 6000×4000 photo resized to 600×400 is uploaded at 1200×800. What follows the resize may only keep pixels as they are (merges, colour and alpha nodes, crops, blurs, glows, shadows) or scale by a literal `transform` scale. The reduced copy is averaged over each pixel's footprint in linear light, on the CPU, before upload. Image textures are cached by identity and decoded size, so a composition that shows an image larger decodes it again.

`ImageFileResolver` decodes a file at its full size first, so a file whose decode needs more than the 512 MiB the decoder allows one image still fails as `media.too_large` before any reduction.

**Breaking, for Rust hosts:** a host that relied on an image past the texture limit failing gets a reduced render and a warning instead, where the image's uses let it be reduced.
