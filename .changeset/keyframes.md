---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-types": minor
"@lumiscia/lumen-shared": minor
---

Expressions gain keyframe tracks: `keyframes(t, keys...)` or `keyframes(t, ease, keys...)`, with one `key(at, value, ease?)` per key. Values are numbers, vec2 values or colours, and colours mix premultiplied like `lerp`. A key's `ease` is how the curve leaves it toward the next key, and keys without one use the track's default, which is `'linear'`. Before the first key and after the last the value holds, and two keys at the same `at` jump.

Easings are now values. An easing is a name such as `'out_expo'`, the new `'hold'`, which keeps the value until the next key and then jumps, or `cubic_bezier(out, in)`, which is CSS `cubic-bezier(out.x, out.y, in.x, in.y)` with `vec2` handles. `tween` takes any easing, and `ease(easing, t)` applies one to progress `t`. In `@lumiscia/lumen-shared`, `ease` is callable as `ease(easing, t)` and still holds the easing names, such as `ease.outExpo`, and `key` and `keyframes` helpers are added.

**Breaking:** `cubic_bezier(x1, y1, x2, y2, t)` no longer parses. Write `ease(cubic_bezier(vec2(x1, y1), vec2(x2, y2)), t)` instead; the parse error says so. The TS helper changes to `cubicBezier(out, in)` with it. `ease`, `key` and `keyframes` are now reserved names, so composition functions and their parameters can't use them.
