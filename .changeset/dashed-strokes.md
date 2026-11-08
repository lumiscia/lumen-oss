---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-types": minor
---

`path` strokes take a dash pattern. `stroke_dash` lists up to 16 non-negative lengths in untransformed pixels that the stroke alternately draws and skips, starting with a dash; an odd number of lengths repeats, as in SVG. Each length takes a literal or an expression. Every dash has round caps, so a dash of length 0 draws a dot, and dashes that cross a corner join as a solid stroke does. `stroke_dash_offset` shifts the pattern along each contour and takes an expression, so `"=frame"` marches the dashes. Dashes are measured along the whole contour, and `trim_start` and `trim_end` then clip them. An empty or all-zero pattern strokes solidly, and paths without one render as before.
