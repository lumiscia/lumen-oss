---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-cli": minor
---

**New: `lumen preview` writes a contact sheet of a composition.** It renders frames sampled evenly across the timeline and tiles them into one `.png` or `.jpg`, so a whole composition can be seen at a glance without rendering a video.

```bash
lumen preview composition.json -o sheet.png
lumen preview composition.json -o sheet.jpg --count 20 --columns 5 --frames 30..300
```

`--count` (default 12) frames are spread from the first frame of `--frames` (default the whole timeline) to its last, `--columns` (default 4) to a row on a sheet `--width` (default 1920) pixels wide, each cell at the composition's aspect ratio over a dark grey background. It also takes `--media-root`, `--font-dir` and `--max-text-glyphs`, as `lumen render` does.
