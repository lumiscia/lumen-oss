---
"@lumiscia/lumen-bindings": minor
---

A text node draws up to 16384 glyphs, strokes and decorations by default instead of 2048, so a full 1920 × 1080 frame of 16 px monospace text (200 × 68 glyphs) is one node. The limit is a host setting: `--max-text-glyphs` (`LUMEN_MAX_TEXT_GLYPHS`) of `lumen`, `--max-text-glyphs` (`LUMEN_SERVER_MAX_TEXT_GLYPHS`) of `lumen-server`, and `GpuCompositionRenderer::set_max_text_glyphs` in `lumen-engine`, up to 1048576. A text over the limit is still a render error before its glyphs are drawn, and the error now names the limit and how a host raises it. Compositions that split text into nodes of under 2048 glyphs render as before.
