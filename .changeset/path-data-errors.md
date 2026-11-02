---
"@lumiscia/lumen-bindings": minor
---

Malformed `path` data now fails the frame with an error naming the node and its `data` field, such as `` `data` is malformed: unexpected data at position 1 ``, instead of rendering nothing. Data that is not finite, such as `1e300` coordinates, is refused the same way, and an empty string is still an empty path. Fill antialiasing now also covers the closing edge of an open path, so it matches the same path ending in `Z`, and negative stroke widths draw no stroke.
