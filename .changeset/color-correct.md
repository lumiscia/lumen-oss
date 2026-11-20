---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-types": minor
---

The new `color_correct` node grades a raster in linear light: white balance (`temperature`, `tint`), per-channel `lift`, `gamma`, `gain` and `offset`, `contrast` around a `pivot`, and tone `curves` (a master curve and one per channel, each up to 16 points joined by a smooth monotone curve). Every param is neutral by default and takes expressions, and the alpha is untouched.

```json
{
  "type": "color_correct",
  "params": {
    "temperature": 0.25,
    "gain": [1.1, 1, 0.95],
    "curves": {
      "master": [
        [0, 0],
        [0.5, 0.62],
        [1, 1]
      ]
    }
  }
}
```
