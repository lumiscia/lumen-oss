---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-types": minor
"@lumiscia/lumen-shared": minor
---

`keyframes` has a `smooth` easing: the curve passes through every key with continuous speed, taking each key's slope from its neighbours, so a path through a few keys no longer kinks at each one. It works for numbers, vec2, vec3 and colours, doesn't overshoot between keys, and mixes with other easings. The SDK's `sampledKeys` samples a function every few frames into a `smooth` track instead of one key per frame.

```ts
sampledKeys((f) => [Math.sin(f / 20) * 100, Math.cos(f / 20) * 100], {
  start: 0,
  end: 120,
  step: 5,
});
// =keyframes(frame, 'smooth', key(0, vec2(0, 100)), key(5, vec2(24.7404, 96.89124)), ...)
```
