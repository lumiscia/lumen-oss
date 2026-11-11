---
"@lumiscia/lumen-bindings": minor
---

Video follows its primaries and transfer tags, and HDR video is tone-mapped to SDR. Until now every video was read as SDR BT.709 whatever it was tagged, so PQ and HLG footage, such as iPhone HDR clips, looked dark and flat, and BT.2020 or P3 footage looked dull. No params change: a tagged source just renders right.

- **Primaries:** BT.2020, Display P3 (SMPTE 432) and DCI-P3 (SMPTE 431, adapted to D65 with the Bradford transform) convert to the working space's BT.709 in linear light. Colour outside BT.709 is clipped at zero as it comes in. BT.709, BT.601 and untagged primaries are unchanged.
- **Transfer:** BT.709, BT.601, BT.2020 SDR and sRGB tags decode with the sRGB curve, as before, so SDR video renders exactly as it did. PQ (SMPTE ST 2084) decodes to nits; HLG (ARIB STD-B67) decodes to scene light and goes through its OOTF for a 1000-nit display with system gamma 1.2.
- **Tone map**, applied only to PQ and HLG sources, as their frames come in. With `L` the display light in nits, after the primaries conversion and clipping:
  1. `m = max(R, G, B)`, and `E = PQ⁻¹(m) / PQ⁻¹(1000)`, capped at 1.
  2. The ITU-R BT.2390 EETF for a 1000-nit source on a 203-nit display: `maxLum = PQ⁻¹(203) / PQ⁻¹(1000) ≈ 0.772`, knee `KS = 1.5 · maxLum − 0.5 ≈ 0.659` (about 88 nits). Below the knee `E' = E`; above it, with `T = (E − KS) / (1 − KS)`, `E' = (2T³ − 3T² + 1)·KS + (T³ − 2T² + T)·(1 − KS) + (−2T³ + 3T²)·maxLum`.
  3. `m' = PQ(E' · PQ⁻¹(1000))`, and the output is `L · (m' / m) / 203`, so 203 nits is SDR white (1.0) before the shoulder and every channel takes the same gain, which keeps hue.

  Mid grey (26 nits) shows at 0.128, as plain scaling gives; 100 nits at 0.49; HDR reference white (203 nits, HLG 75%) at 0.78, sRGB code 229; 500 nits at 0.98; 1000 nits and brighter at 1.0. The curve is static: no per-frame adaptation and no mastering metadata.

- **Matrix:** BT.2020 constant-luminance video fails with `media.unsupported` (re-encode with `-colorspace bt2020nc`) instead of decoding wrong.
- **New warning `media.color_unsupported`:** a video tagged with another transfer (such as `log100` or `gamma22`) or primaries (such as `film`) renders as SDR BT.709 and says so once per composition, `media_in` node and source.
- 10-bit and 12-bit samples keep their precision into the working space on every decode path.

These conversions apply to CPU decode and both hardware decode paths, Vulkan Video and VideoToolbox, which share one shader. The browser preview still reads video as SDR BT.709 past its YUV matrix; matching it to renders comes with ICC support.

**Rust API:** `lumen_ffmpeg::ColorTags` gains `primaries`, `transfer` and `chroma_location`, with `ColorPrimaries`, `ColorTransfer` and `ChromaLocation`, and `ColorTags::new(matrix, full_range)`; `VideoStreamInfo` has the stream's `color`. `lumen_engine::media::YuvColor` gains `primaries: ColorPrimaries` and `transfer: TransferFunction`, and `YuvColor::sdr(matrix, full_range)` builds the SDR BT.709 case. `VideoFrameResolver::color_warning` (default `None`) reports tags a resolver reads without. Code that builds these structs by their fields needs the new fields.

RGB and grey video (FFV1 `gbrp`, for example) decodes to 8-bit RGBA read as sRGB, so its primaries and transfer are not converted; an HDR or wide-gamut stream of that kind gets the same `media.color_unsupported` warning. The warning is decided from the stream's tags when it opens: a frame whose own tags differ from the stream's is converted by its own tags but does not change the warning.
