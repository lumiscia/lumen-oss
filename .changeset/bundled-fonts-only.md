---
"@lumiscia/lumen-bindings": minor
---

Native renders no longer read the machine's installed fonts, so text renders the same on every machine, as it already did in the browser. A text node's font family must be loaded through the media store, or it falls back to the bundled Roboto and any other loaded fonts: `lumen-local` resolves families from its font directories and the system, and `lumen-server` from its media and font roots. Characters a font lacks, such as CJK, Hebrew or emoji, fall back only to loaded fonts, chosen the same way on every platform. Load a font that covers them instead. `lumen-bench-text` takes `--font PATH` to load fallback fonts for its emoji and mixed-script cases.
