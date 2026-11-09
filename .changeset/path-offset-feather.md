---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-types": minor
---

`path` fills take an `offset` and a `feather`, both in untransformed pixels and both animatable. A positive `offset` grows the fill outward, rounding the corners it grows around, and a negative one shrinks it; a fill shrunk past its middle vanishes, and the stroke stays on the original outline. `feather` fades the fill's edge over that width, centred on the edge, without a blur pass. Under non-uniform scale both stretch with the path, as `stroke_width` does. At 0, the default, fills render as before, and an offset or feather animated to 0 meets them there. Edges that coincide and cancel under the even-odd rule, such as the side two adjoining squares share, are no longer edges of the fill: they are neither offset nor feathered, and sharp fills no longer show a faint seam along them.
