---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-types": minor
"@lumiscia/lumen-shared": minor
---

Expressions can write numbers as text with a fixed number of decimals and padding, and join text from parts, so a frame counter, timecode or coordinate readout follows the frame.

`format(value, decimals?, width?, fill?)` gives `decimals` digits after the point (default 0), then pads on the left with the one-character `fill` (default a space) to `width` characters (default 0). Halves round away from zero on the number as written, so `format(2.5)` is `"3"`, `format(-2.5)` is `"-3"`, `format(0.125, 2)` is `"0.13"` and `format(1.005, 2)` is `"1.01"`; a result that rounds to zero never prints a minus. With a `"0"` fill the sign stays first: `format(-5, 0, 4, "0")` is `"-005"`. `value` must be finite, `decimals` a whole number from 0 to 20 and `width` one from 0 to 64. `concat(a, b, ...)` joins strings and numbers, with numbers shown as a text param shows them (`concat("f ", frame / 7)` reads like `=frame / 7`); vec2, vec3 and colour parts are errors. `+` stays numeric.

```json
{
  "type": "text",
  "params": { "content": "=concat('frame ', format(frame, 0, 4, '0'), ' / 1200')" }
}
```

```ts
c.addNode({
  type: "text",
  params: { content: concat("frame ", format(frame, 0, 4, "0"), " / 1200") },
});
```

In the TypeScript builder, `format` and `concat` are `Expr<string>`, and a text node's `content` and a span's `text` take `Expr<number>` as well, so `content: round(frame.mul(3.7))` type-checks.

`format` and `concat` are now reserved names: a composition function or parameter called either needs another name.
