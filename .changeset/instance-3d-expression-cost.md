---
"@lumiscia/lumen-bindings": patch
---

An `instance_3d` evaluates its per-copy params faster. `position`, `rotation` and `scale` are evaluated as one plan, so a pure term two of them share is evaluated once per copy, and a pure part that reads neither `instance` nor `instance_count`, such as a `keyframes` track every copy follows, is evaluated once for all copies. `&&`, `||` and `if` stay lazy, so a part in a branch no copy takes still never runs, and a failure still names the copy it happened in. Expressions also allocate less while they evaluate. A part that reads another node, measures text or reads a template prop is evaluated for each expression as it would be alone. Nothing renders differently.
