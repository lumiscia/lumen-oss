---
"@lumiscia/lumen-bindings": patch
---

The browser bindings reserve 16 MiB of WebAssembly linear memory at startup instead of 128 MiB, growing on demand up to the existing 2 GiB ceiling. Typical compositions stay at 16 MiB during checks (larger inputs grow on demand), and playback uses less memory up front without slowing down.
