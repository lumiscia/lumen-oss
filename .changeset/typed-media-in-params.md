---
"@lumiscia/lumen-bindings": minor
---

`media_in` video ranges now do what they say. `range_end` is the first frame past the range and is optional, so leaving it out plays to the end of the source, and `range_start` is honoured on its own. Before, `range_end: 0` meant "no range" and ignored `range_start`. An expression for `kind` or `loop_mode` that doesn't name a value, such as `=2` for `kind`, now fails instead of becoming `image` or `clamp`. A `media_in` whose source can't be found or opened fails when the composition is compiled, with the reason, instead of at the first frame.
