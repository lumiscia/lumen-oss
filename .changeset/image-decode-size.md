---
"@lumiscia/lumen-bindings": minor
---

Images are decoded no larger than they are shown.

An image larger on a side than the device's texture limit now renders from a copy reduced to fit, with a `media.image_reduced` warning, instead of failing as `media.too_large`. `GpuCompositionRenderer::set_max_image_side` sets a lower limit of the host's own.

An image every use of which is drawn through a `resize` with a literal `width`, `height` and `mode`, in the root composition, and shown at under half its size, is decoded at twice the size it is shown: a 6000×4000 photo resized to 600×400 is uploaded at 1200×800. What follows the resize may only keep pixels as they are (merges, colour and alpha nodes, crops, blurs, glows, shadows) or scale by a literal `transform` scale. Any other use keeps the image at its own size. The reduced copy is averaged over each pixel's footprint in linear light, on the CPU, before upload. Image textures are cached by identity and decoded size, so a composition that shows an image larger decodes it again.

**Breaking, for Rust hosts:** `MediaTextureKey::of_image` is gone; a key is the image's identity and the size it is decoded at. A host that relied on an image past the texture limit failing gets a reduced render and a warning instead.
