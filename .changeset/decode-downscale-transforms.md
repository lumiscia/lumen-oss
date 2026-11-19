---
"@lumiscia/lumen-bindings": minor
---

Images that only `resize`, `transform` and `image_plane_3d` nodes read are decoded at the size they're shown at, and fit the texture limit instead of failing.

Previously only an image whose every use was a literal `resize` could be reduced, and only to an exact multiple of its aspect ratio. Now these three nodes read a reduced image at its density, so it keeps its size and placement on the canvas, and its reduction is per axis. A 1000×9 strip can be reduced too.

- **Shown smaller:** a `transform` with a literal scale bounds the size it shows its source at, as a literal `resize` does. A photo shown at a quarter by `scale_x: 0.25, scale_y: 0.25` is uploaded at half its size. Rotation doesn't matter.
- **Past the limit:** an image larger on a side than the device's texture limit, or `GpuCompositionRenderer::set_max_image_side`, now renders from a reduced copy with a `media.image_reduced` warning when only these nodes read it. That includes a `transform` whose scale is an expression and an `image_plane_3d`. Any other reader, such as a blur, a merge, a shader or the graph's output, keeps the image at its own size, and past the device's limit it still fails as `media.too_large`.

**Breaking, for Rust hosts:** an image that a `transform` or `image_plane_3d` reads, past the texture limit, now renders reduced with a warning instead of failing. Past `set_max_image_side`, it is now reduced instead of kept at its own size.
