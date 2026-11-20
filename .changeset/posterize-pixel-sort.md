---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-types": minor
---

Two looks that needed hand-written shaders are now nodes. `posterize` quantizes each channel to `levels` steps in sRGB, or with `mode: "palette"` gives each pixel the colour of a 2 to 16 colour `colors` list whose brightness band it falls in, with an optional Bayer `dither` that holds still when the source moves. `pixel_sort` sorts runs of pixels along rows or columns by `luma`, `hue` or `saturation`, within a `low` to `high` band, in segments of at most `max_length` pixels (up to 1024) so the cost stays bounded.

```json
{
  "type": "posterize",
  "params": {
    "mode": "palette",
    "colors": [
      [8, 8, 8, 255],
      [191, 20, 20, 255],
      [201, 209, 214, 255]
    ]
  }
}
```
