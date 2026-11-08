---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-types": minor
---

Radial gradients take an optional `focal` point, which makes them two-point conical gradients for off-centre highlights. The first stop sits at the focal point and the last on the circle at `center` and `radius`. A focal point on or outside that circle is moved just inside it. Without `focal`, or with `focal` equal to `center`, gradients render as before.
