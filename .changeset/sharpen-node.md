---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-types": minor
---

New `sharpen` node: an unsharp mask. It adds back `amount` times the difference between each pixel's luminance and that of a copy blurred by `radius`, ignoring contrast under `threshold`. The result is held to the range of the pixel's 3×3 neighbours, so hard edges get no halos, and the change is added equally to red, green and blue, so there are no colour fringes.

```ts
nodes.sharpen({ amount: 1.5, radius: 2, threshold: 0.02, source: photo });
```
