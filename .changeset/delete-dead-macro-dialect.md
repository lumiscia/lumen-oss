---
"@lumiscia/lumen-bindings": patch
---

Removed the unused `#[property(...)]` field attribute from `#[derive(Node)]` and the `#[node_impl]` attribute macro from `lumen-engine-macros`. No node used either, and the generated node definitions are unchanged. Node parameters are declared with `#[params]` and `#[derive(Delegate)]`.
