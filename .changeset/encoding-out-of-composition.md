---
"@lumiscia/lumen-types": minor
"@lumiscia/lumen-shared": minor
"@lumiscia/lumen-sdk": minor
"@lumiscia/lumen-cli": minor
"@lumiscia/lumen-bindings": minor
---

**A composition no longer holds encoding settings.** `codec`, `quality`, `bit_depth` and `alpha` are gone from `render_settings`, and a document that still has them is rejected as an unknown field. How a video is encoded is decided by the render request, then by the codec the output container holds (`prores` in a `.mov`, `vp9` in a `.webm`), then by the host's defaults. Every surface already took these options, so move them there; `lumen render comp.json -o out.mov` with no flags still picks ProRes.

Before:

```json
{
  "render_settings": {
    "width": 1920,
    "height": 1080,
    "codec": "hevc",
    "quality": "high",
    "bit_depth": 10
  }
}
```

After, a composition with only the canvas, and the encoding on the render:

```json
{ "render_settings": { "width": 1920, "height": 1080 } }
```

```bash
lumen render comp.json -o out.mp4 --codec hevc --quality high --bit-depth 10
```

```ts
await lumen.render(composition, { codec: "hevc", quality: "high", bitDepth: 10 });
await render(composition, { output: "out.mp4", codec: "hevc", quality: "high", bitDepth: 10 });
```

`new Composition({ renderSettings })` takes only `width` and `height`. `VideoCodec` and `VideoBitDepth` stay exported from `@lumiscia/lumen-types`, which the SDK and the `lumen` wrapper now import them from.

**Rust API:** `lumen_engine::composition::{OutputCodec, OutputSettings, BitDepth}` and `Composition::output_settings()` are removed; use `lumen_ffmpeg::{EncodeCodec, BitDepth}`. The engine's GPU encode targets take `lumen_engine::gpu::BitDepth`, which converts from `lumen_ffmpeg::BitDepth` with `From`. `lumen_server::render::resolve_encoder` reads only the request, the container and the host's config.
