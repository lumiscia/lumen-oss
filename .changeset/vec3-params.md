---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-types": minor
"@lumiscia/lumen-shared": minor
---

Params can now hold three numbers, written `[x, y, z]` in JSON or as an expression, ready for the 3D nodes' `position`, `rotation` and `scale`. Expressions gain `vec3(x, y, z)`, and vec3 values work like vec2 values: they add and subtract with each other, multiply and divide componentwise or by a number on either side, negate, and interpolate in `lerp`, `tween` and `keyframes`, e.g. `"=tween(vec3(0, 0, -400), vec3(0, 0, 0), 0, 24, 'out_expo') + vec3(0, 10 * sin(time), 0)"`. Mixing a vec3 with a vec2 is an error. Template props can be declared `"type": "vec3"`, and a `wgsl_shader` vec3 uniform takes a vec3 expression. In `@lumiscia/lumen-shared` the helper is `vec3`, which returns an `Expr<Vec3Value>` that TypeScript keeps apart from colour expressions, so a vec3 can't reach a colour param or share a `lerp` or keyframe track with a colour. `prop.vec3(..)` declares a prop, and params and prop defaults take a plain `Vec3` tuple `[x, y, z]`. A three-number tuple passed to a helper still means `rgb(..)`, so write `vec3(x, y, z)` there. `vec3` is now a reserved name, so composition functions and their parameters can't use it.
