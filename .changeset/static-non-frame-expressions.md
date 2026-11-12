---
"@lumiscia/lumen-bindings": patch
"@lumiscia/lumen-shared": patch
---

A node whose expressions can't change from frame to frame is kept across frames like one with literal params. `"=vec3(0, 0, 520)"` or `"=max(0.9, 0.5)"` no longer redraws its node, and everything downstream, on every frame. An expression counts as changing when it reads `frame` or `time`, calls `progress` or `tween`, or reads another node, a template prop or measured text, as an `instance_3d` already counted.

`vec2(..)` and `vec3(..)` of plain numbers are written to a node's params as the literal `[x, y]` or `[x, y, z]`. They are still expressions, so `vec2(1, 2).add(..)` works and `String(vec2(1, 2))` is `"=vec2(1, 2)"`.

A number written in a composition's JSON is now read exactly, so `[0.9999999999999999, 0]` compares equal to `vec2(0.9999999999999999, 0)` as it does when the vector is written as an expression.
