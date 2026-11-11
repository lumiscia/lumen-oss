---
"@lumiscia/lumen-preview": minor
"@lumiscia/lumen-bindings": minor
---

The preview converts colour as a render does. Images are decoded with their embedded colour profile honoured and converted to sRGB explicitly (`createImageBitmap` with `colorSpaceConversion: "default"`, read back through an sRGB 2D canvas), whatever the display, so a Display P3 photo looks as it does in a render within 8-bit rounding. Video frames handed to the engine as YUV planes now carry the source's tagged primaries and transfer, so BT.2020 and Display P3 video is converted to BT.709 and PQ and HLG video is tone-mapped to SDR as a render does, instead of being read as SDR BT.709. A YUV tag the engine doesn't convert, such as a `linear` transfer or unknown primaries, warns once per source with `media.color_unsupported`, as a render does. Video frames the browser hands over as RGB in other primaries or another transfer are drawn on an sRGB canvas, which converts them; sRGB, BT.709 and BT.601 ones are copied as they are.

The preview is still not pixel-identical to a render. 16-bit images arrive in the browser at 8 bits, and colour outside sRGB is clipped there. Images pass through an 8-bit canvas that stores colour premultiplied, so straight colour at very low alpha is coarsely rounded, as before. Browsers' decoders and colour management differ, by a code or so.

`setVideoFrameYuv` on the preview's wasm controller takes two more optional arguments at the end, `primaries` and `transfer`, the source's WebCodecs tags; a host calling it directly that leaves them out gets BT.709 SDR as before.
