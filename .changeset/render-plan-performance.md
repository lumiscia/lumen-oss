---
"@lumiscia/lumen-bindings": patch
---

Reduce render preparation and per-frame CPU overhead by recording consecutive GPU compute operations in shared passes, avoiding repeated text-atlas lookups and using faster hashes for binding maps and content fingerprints. Load all literal font families before recording the font generation in the compiled-plan key, preventing a redundant plan compile on the first text frame. These changes preserve the composition format and rendered output.
