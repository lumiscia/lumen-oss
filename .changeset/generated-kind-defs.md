---
"@lumiscia/lumen-types": minor
---

The composition schema's paint, shader uniform and clip prop `$defs` are now generated from the Rust param definitions, with the same accepted values as before. New `$defs` name the shapes that were inline: `gradientPaint`, `gradientStop`, `paintKind`, `gradientUnits`, `gradientSpread`, `gradientInterpolation`, `shaderUniformComponent`, `shaderUniforms` and `clipProps`; gradient fields carry descriptions, defaults and enum options. `@lumiscia/lumen-types` generates `Paint`, `GradientPaint`, `GradientStop`, `ShaderUniforms`, `ShaderUniformValue`, `ShaderUniformComponent`, `ClipProps` and `ClipPropValue` from these `$defs`, with JSDoc, and exports the gradient enums as `PaintKind`, `GradientUnits`, `GradientSpread` and `GradientInterpolation`.

For Rust: `GradientPaintDelegate` and `GradientStopDelegate` are exported from `vector::paint`, and the gradient enums implement `NodeEnum` with an `enum` property kind.
