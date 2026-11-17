---
"@lumiscia/lumen-bindings": patch
"@lumiscia/lumen-cli": patch
---

`lumen render --frame` takes a comma-separated list of frames for an output with `%d`, and writes one still per frame from a single load of the composition, fonts and media. Frames render in the order given, each file is named by its own frame number padded as the largest frame is, and a frame listed twice fails. A single value, and an output without `%d`, behave as before.

```sh
lumen render film.json -o shots/frame-%d.png --frame 0,120,348,610
```
