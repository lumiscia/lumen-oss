---
"@lumiscia/lumen-bindings": minor
---

Malformed `path` data now fails the frame with an error naming the node and its `data` field, such as `` `data` is malformed: unexpected data at position 1 ``, instead of rendering nothing. Coordinates and radii beyond +/-1e7, and arcs too extreme to flatten, are refused the same way, and an empty string is still an empty path. Fill antialiasing now also covers the closing edge of an open path, so it matches the same path ending in `Z`, and a stroke width of zero or less draws no stroke.
