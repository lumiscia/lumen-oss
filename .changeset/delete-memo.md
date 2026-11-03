---
"@lumiscia/lumen-bindings": minor
---

Removed the `memo` node. Static branches are already reused across frames without it, and a branch that depends on the frame is rendered every frame. A composition that still uses `memo` is rejected with that message. The snapshots it held, and the plan pinning and cross-plan copies that kept them, are gone with it.
