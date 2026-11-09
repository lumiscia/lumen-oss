---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-types": minor
"@lumiscia/lumen-shared": minor
---

Expressions gain `path_point(path_data, t)` and `path_angle(path_data, t)` for moving along a curve, e.g. `"position": "=path_point('M 0 0 C 100 0 100 100 200 100', t)"` with `"rotation": "=path_angle(..)"`. `t` is the fraction of the path's arc length, clamped to `0..=1`, so a steady `t` moves at a steady speed. `path_point` returns a vec2 on the curve; `path_angle` is the direction of travel in degrees clockwise from +x, like `rotation`, and counts whole turns instead of wrapping, so a clockwise circle ends at 360. Moves between contours add no length, and a closed contour's closing segment does. Path data is parsed like a `path` node's `data`, and each path is measured once and cached. In `@lumiscia/lumen-shared` they are `pathPoint` and `pathAngle`.
