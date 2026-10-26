---
"@lumiscia/lumen-shared": minor
---

Add typed expression helpers that build `=...` expression strings: one helper per expression builtin (`tween`, `vec2`, `rgb`, `spring`, `outExpo`, ...), `ease.*` easing names, `sec`/`ms` units, `hex` colours, the `frame`/`time`/`fps`/`width`/`height` globals, and `.add`/`.sub`/`.mul`/`.div`/`.oneMinus`/`.clamp` with the evaluator's vec2 and colour rules. `fn("rise(start)", ...)` defines a composition function, which `Composition.addFunction` adds to `functions`, and `moves(base, [...])` sums tweens. Node params accept these expressions directly.
