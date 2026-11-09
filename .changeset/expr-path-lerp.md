---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-types": minor
"@lumiscia/lumen-shared": minor
---

`lerp`, `tween` and `keyframes` interpolate path data, so a `path` node's `data` can morph: `"=lerp('M 0 0 L 100 0 L 50 80 Z', 'M 0 40 L 100 40 L 50 0 Z', ease('in_out_cubic', t))"`, or `"=keyframes(frame, key(0, 'M 0 0 L 100 0 L 50 80 Z'), key(30, 'M 0 40 L 100 40 L 50 0 Z'))"`. The two paths must have the same commands in the same order, with relative and absolute commands told apart and an arc's flags matching, and they interpolate point by point; otherwise the error names the first command that differs. Paths are not resampled to match. The result writes each command out, with numbers at the precision the `path` node draws with, and numbers that both paths share aren't mixed, so they don't jitter. Keys that don't interpolate, a `hold` or two keys at the same time, may switch between paths of any structure. Strings that aren't path data don't interpolate, and the error says which one is malformed. Literal paths are checked when the expression is parsed. In `@lumiscia/lumen-shared`, `lerp`, `tween` and `key` accept strings.
