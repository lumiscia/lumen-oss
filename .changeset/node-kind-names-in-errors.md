---
"@lumiscia/lumen-bindings": patch
"@lumiscia/lumen-types": patch
---

Render errors name the node by its JSON `type`, like `media_in node 3 evaluation failed`, instead of a Rust-style `MediaIn`. `motion_blur` now says in its description that it blurs content that already has motion blur a second time, and `lumen-local --help` no longer reads as if a quality preset targeted 14 Mb/s.
