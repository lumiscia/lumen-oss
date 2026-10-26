---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-types": minor
"@lumiscia/lumen-shared": minor
---

`wgsl_shader` now takes up to 16 inputs (`input0..input15` plus a shared `input_sampler`), gives every shader `lumen.time`, `lumen.frame` and `lumen.resolution`, binds typed uniforms by name from the shader's uniform struct, rebuilds its pipeline when the source changes, premultiplies straight-alpha inputs before the shader sees them, and rejects unknown or malformed uniform values and shader bindings above 2 instead of ignoring them.

Uniforms are set with a `uniforms` object instead of the `bindings` string, which is removed. Each value is a number, an array of 2-4 numbers or `=expression`s, a whole-value `=expression`, or, for `vec3<f32>` and `vec4<f32>` fields, a `#rrggbb` / `#rrggbbaa` colour or a colour expression such as `=rgb(255, 128, 0)`. `wgslShader()` values also take typed `Expr`s from the expression helpers. The schema and the generated `ShaderUniforms` / `ShaderUniformValue` types describe it, and `wgslShader()` in `@lumiscia/lumen-shared` declares the uniform struct in TypeScript and type-checks each field's value.

The GPU renderer now keeps compiled plans within an estimated memory budget (1 GiB by default, configurable with `GpuCompositionRenderer::set_plan_cache_budget`), dropping least recently used plans instead of keeping one per animated switch or shader combination.
