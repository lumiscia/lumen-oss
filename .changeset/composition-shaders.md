---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-types": minor
"@lumiscia/lumen-shared": minor
---

A composition can define a WGSL shader once in a top-level `shaders` map and use it from any `wgsl_shader` node, in the root or any template, as `shader:<name>`; an expression may pick between names, such as `=if(frame < 120, 'shader:grain', 'shader:halftone')`. Each entry, and each distinct inline source, is checked once when the composition loads, so a broken shader is one error, at `/shaders/<name>` or at the first node holding it, listing every node that uses it. Rendering finds each node's shader by a handle instead of copying and hashing its source every frame, so existing compositions that repeat an inline source get the faster bind too. In the SDK, `wgslShader({ name, shader, uniforms })` makes nodes that emit `shader:<name>` and registers the source in `shaders` once, including through templates.

```ts
const grain = wgslShader({ name: "grain", uniforms: { amount: "f32" }, shader: grainSource });
composition.addNode(grain.node({ amount: 0.3 })); // params.shader is "shader:grain"
```
