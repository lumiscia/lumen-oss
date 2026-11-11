---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-types": minor
---

**Breaking: the compositor works in linear light.** Every raster is now `Rgba16Float`, linear with BT.709 primaries and premultiplied alpha, with 1.0 as SDR white and room above it. JSON colours, images and video are sRGB-encoded and decode on the way in; `media_output` composites over black for opaque targets, encodes back to sRGB and quantises once. An 8-bit image passed straight through comes out byte for byte, and JSON colours render as the codes they name.

**Pixel output changes** wherever colours mix:

- Fades, normal merges, masks, blurs, glow, shadows, motion blur, transform and resize sampling, and antialiased edges of shapes and text mix light, not sRGB codes. 50% white over black is code 188, not 128, and a blurred black-white edge is brighter. Small dark text on a light background looks lighter, and light text on dark looks bolder.
- A luma matte is the linear BT.709 luminance of the mask.
- Non-normal blend modes (`multiply`, `screen`, `overlay`, ...), `levels`, `hue_saturation`, the `color_grade` LUT, `exposure`'s contrast and offset, `displacement_map` and `fractal_noise` still work on sRGB-encoded values, so mid grey stays 128 and they look as they did, but what they composite onto is now linear. `exposure` multiplies linear light by 2^stops.
- 8-bit output is ordered-dithered below half a code step, so flat colours keep their exact codes and smooth gradients lose their banding. The CPU, Metal and Vulkan Video encoders and the NVENC path dither; `GpuCompositionRenderer::set_output_dither(false)` turns it off.
- Straight-alpha images are premultiplied on the GPU in linear light, so translucent PNG edges no longer darken.

**Migrating a `wgsl_shader`:** inputs are linear and premultiplied, and values above 1 are kept. The output, and any storage texture you declare, must be `texture_storage_2d<rgba16float, write>`; a shader declaring `rgba8unorm` fails with a message saying so. Colour uniforms arrive decoded to linear and premultiplied; numbers and vectors arrive as written. The prelude has `lumen_srgb_to_linear` and `lumen_linear_to_srgb` for formulas that need encoded values:

```wgsl
let color = textureLoad(input, pixel, 0);
let encoded = lumen_linear_to_srgb(color.rgb / max(color.a, 1e-6));
let graded = my_curve(encoded); // a formula written for sRGB codes
textureStore(output, pixel, vec4(lumen_srgb_to_linear(graded) * color.a, color.a));
```

**Rust API:** `MediaFrame::RawRgba` holds straight colour, which images now use; `premultiply_rgba_in_place_if_needed` is gone. `VulkanVideoTarget::nv12` and `CudaNvencTargetPool::new` take a `dither` flag, and both targets render from `Rgba16Float`. `GpuCompositionRenderer::set_opaque_output(true)` composites over black for targets without alpha; the server, bench and browser preview set it for opaque outputs.
