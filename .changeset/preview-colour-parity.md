---
"@lumiscia/lumen-preview": minor
"@lumiscia/lumen-bindings": minor
---

The preview's colour matches a render's. Images are decoded with their embedded colour profile honoured and converted to sRGB explicitly (`createImageBitmap` with `colorSpaceConversion: "default"`, read back through an sRGB 2D canvas), whatever the display, so a Display P3 photo looks as it does in a render within 8-bit rounding. Video frames handed to the engine as YUV planes now carry the source's tagged primaries and transfer, so BT.2020 and Display P3 video is converted to BT.709 and PQ and HLG video is tone-mapped to SDR as a render does, instead of being read as SDR BT.709. Video frames copied as RGB ask the browser for sRGB.

16-bit images still arrive in the browser at 8 bits, and colour outside sRGB is clipped there.

`LumenPreviewController.setVideoFrameYuv` takes two more optional arguments at the end, `primaries` and `transfer`, the source's WebCodecs tags; a host calling it directly that leaves them out gets BT.709 SDR as before.
