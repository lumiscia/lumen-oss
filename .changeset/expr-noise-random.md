---
"@lumiscia/lumen-bindings": minor
---

Expressions gain seeded `noise(seed, x)` and `random(seed, x)` builtins. `noise` is smooth 1D gradient noise in `-1..=1`; `random` is a uniform number in `0..1`, and `random(x)` uses seed 0. Both are built from integer hashing and exact float arithmetic, so a frame renders identically on every host.
