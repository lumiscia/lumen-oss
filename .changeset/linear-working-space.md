---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-types": minor
---

**Breaking: the compositor works in linear light.** Every raster is now `Rgba16Float`, linear with BT.709 primaries and premultiplied alpha, with 1.0 as SDR white and room above it. JSON colours, images and video are sRGB-encoded and decode on the way in; `media_output` composites over black for opaque targets, encodes back to sRGB and quantises once. An 8-bit image passed straight through comes out byte for byte, and JSON colours render as the codes they name.

**Pixel output changes** wherever colours mix:

- Fades, normal merges, masks, blurs, glow, shadows, motion blur, transform and resize sampling, and antialiased edges of shapes and text mix light, not sRGB codes. 50% white over black is code 188, not 128, and a blurred black-white edge is brighter. Small dark text on a light background looks lighter, and light text on dark looks bolder.
- A luma matte is the linear BT.709 luminance of the mask.
- Non-normal blend modes (`multiply`, `screen`, `overlay`, ...), `levels`, `hue_saturation`, the `color_grade` LUT, `exposure`'s contrast and offset, `displacement_map` and `fractal_noise` still work on sRGB-encoded values, so mid grey stays 128 and they look as they did, but what they composite onto is now linear. `exposure` multiplies linear light by 2^stops.
- 8-bit output is ordered-dithered across the whole code step, so a flat area of any value keeps its mean to 1/64 of a code and smooth gradients lose their banding. A value within a tenth of a code of one keeps that code, so 8-bit content and JSON colours come out exactly. The CPU, Metal and Vulkan Video encoders and the NVENC path dither; `GpuCompositionRenderer::set_output_dither(false)` turns it off.
- Quantised colour never exceeds quantised alpha, and a pixel whose alpha rounds to 0 is transparent black.
- Opaque outputs, video and JPEG, composite over black in linear light: half-covered red is code 188, not 128. PNG stills keep straight alpha, quantised from the straight colour.
- Straight-alpha images are premultiplied on the GPU in linear light, so translucent PNG edges no longer darken. Premultiplied YUV video keeps colour brighter than its alpha.
- Motion blur sums its samples in 32-bit float, so a still source comes through unchanged at any sample count. Glow keeps its light above 1 with alpha at most 1. A `wgsl_shader`'s output has its alpha clamped to `0..=1`, and a texel with a NaN channel is transparent black.

**Migrating a `wgsl_shader`:** inputs are linear and premultiplied, and values above 1 are kept. The output, and any storage texture you declare, must be `texture_storage_2d<rgba16float, write>`; a shader declaring `rgba8unorm` fails with a message saying so. Colour uniforms arrive decoded to linear and premultiplied; numbers and vectors arrive as written. The prelude has `lumen_srgb_to_linear` and `lumen_linear_to_srgb` for formulas that need encoded values:

```wgsl
let color = textureLoad(input0, vec2<i32>(id.xy), 0);
let encoded = lumen_linear_to_srgb(color.rgb / max(color.a, 1e-6));
let curved = encoded * encoded * (3.0 - 2.0 * encoded); // a formula written for sRGB codes
textureStore(output, vec2<i32>(id.xy), vec4<f32>(lumen_srgb_to_linear(curved) * color.a, color.a));
```

**Rust API:** `MediaFrame::RawRgba` holds straight colour, which images now use; `premultiply_rgba_in_place_if_needed` is gone. `VulkanVideoTarget::nv12` and `CudaNvencTargetPool::new` take a `dither` flag, and both targets render from `Rgba16Float`. `GpuCompositionRenderer::set_output_alpha` takes an `OutputAlpha`: `Premultiplied` (the default), `Straight`, or `Opaque`, which composites over black for targets without alpha; the server, bench and browser preview set it per output. A float output format takes the working space unchanged, linear and premultiplied.
