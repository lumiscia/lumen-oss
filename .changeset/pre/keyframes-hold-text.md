---
"@lumiscia/lumen-bindings": patch
---

`keyframes` keys can hold text. A key whose value is a string that isn't path data is accepted wherever its segment eases with `hold`, either the key's own easing or the `keyframes` default when the key has none, so a readout can step between values:

```
=keyframes(frame, 'hold', key(0, 'ten'), key(30, 'hundred'), key(60, '1,024'))
```

Text can't interpolate, so any other easing out of a text key fails with an error that names the key and says text keys need `hold`. Path data keys interpolate as before.
