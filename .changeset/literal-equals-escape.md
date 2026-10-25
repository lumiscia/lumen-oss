---
"@lumiscia/lumen-bindings": patch
---

Allow string params to start with a literal `=` by escaping it as `==`: `"==wow"` renders `=wow`, while `"=frame"` is still an expression.
