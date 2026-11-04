---
"@lumiscia/lumen-bindings": patch
---

Native video sources open their decoder once instead of twice, so a source opens faster and the reported decode mode is the one the decoder actually uses. Rendered frames are unchanged.
