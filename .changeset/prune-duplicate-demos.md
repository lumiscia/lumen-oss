---
"@lumiscia/lumen-bindings": patch
---

`lumen-bench-json-parse` no longer has an `antialiasing_worst_cases_noaa` fixture; `antialiasing_worst_cases_aa` is the remaining antialiasing-worst-cases fixture. The `crates/local/demo` folder drops the duplicated no-AA, conic-angle and gradient-text scenes, and `feature-showcase.json` no longer carries an unconnected `boolean` node.
