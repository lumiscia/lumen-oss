# @lumiscia/lumen-bindings

## 0.6.0-beta.2

### Minor Changes

- eec0701: **New: `lumen preview` writes a contact sheet of a composition.** It renders frames sampled evenly across the timeline and tiles them into one `.png` or `.jpg`, so a whole composition can be seen at a glance without rendering a video.

  ```bash
  lumen preview composition.json -o sheet.png
  lumen preview composition.json -o sheet.jpg --count 20 --columns 5 --frames 30..300
  ```

  `--count` (default 12) frames are spread from the first frame of `--frames` (default the whole timeline) to its last, `--columns` (default 4) to a row on a sheet `--width` (default 1920) pixels wide, each cell at the composition's aspect ratio over a dark grey background. It also takes `--media-root`, `--font-dir` and `--max-text-glyphs`, as `lumen render` does.

- 7a3a9e9: `lumen render` and `lumen check` read the composition from stdin when it is `-`, so a script can pipe one in without writing a file:

  ```bash
  node composition.js | lumen render - -o out.mp4
  cat composition.json | lumen check - --format json
  ```

  Messages and the `lumen check` summary call the source `<stdin>`. Media is found under `--media-root`, which defaults to the current directory. Empty input, or stdin that is a terminal, fails with an error instead of waiting. A file named `-` is reached as `./-`.

### Patch Changes

- @lumiscia/lumen-preview@0.6.0-beta.2

## 0.6.0-beta.1

### Patch Changes

- ce70b5c: `image_plane_3d` takes `region`, `[x, y, width, height]` in its source's pixels, to show only that part of a raster on a rectangle that covers just that part.

  A plane with a region is where it would be and as big as it would be with the whole source on it, so it looks as a plane showing a `crop` of that region does and the two can be swapped. `width` is still the width of the whole source, and `position`, `rotation` and `scale` pivot about the whole source's centre. A region is cut off at the source's edges as `crop`'s is. Each number takes an integer or an expression; the whole `region` cannot be an expression. Unset shows the whole source, as before.

  Planes with regions of one source share its texture and its mip levels and draw together, so a region costs nothing beyond the plane, even one that changes every frame. A `crop` before each plane makes a raster the size of the source and a draw for every piece, on every frame it changes. Cutting a 1080p raster into 63 planes at 120 frames takes 0.6 s, holds 204 MB and submits 2 passes a frame, where 63 crops took 7.2 s, held 1.2 GB and submitted 64.

  ```json
  {
    "id": 4,
    "type": "image_plane_3d",
    "params": { "width": 16, "region": [320, 180, 640, 360], "position": [0, 0, -2] }
  }
  ```

  Sampling is kept inside the region, so what is outside it isn't meant to show, however small the plane is drawn. The price is under strong minification: the band along each edge of the region that is inside the source, and all of a narrow region, is sampled from a sharper mip level than its footprint, because a texel of a shared level can hold pixels from both sides of the edge. That band can alias or shimmer where a `crop` would be smooth.

- 2a1b3bc: An `instance_3d` evaluates its per-copy params faster. `position`, `rotation` and `scale` are evaluated as one plan, so a pure term two of them share is evaluated once per copy, and a pure part that reads neither `instance` nor `instance_count`, such as a `keyframes` track every copy follows, is evaluated once for all copies. `&&`, `||` and `if` stay lazy, so a part in a branch no copy takes still never runs, and a failure still names the copy it happened in. Expressions also allocate less while they evaluate. A part that reads another node, measures text or reads a template prop is evaluated for each expression as it would be alone. Nothing renders differently.
- 3c2dc0f: An `instance_3d` whose params don't read the frame places its copies once and keeps their matrices, so a later frame only places them under what holds the node: a still field of copies under a moving camera, light or parent no longer evaluates `position`, `rotation` and `scale` for every copy on every frame. The kept matrices are dropped when the composition changes, or after a frame that doesn't draw the node. Nothing renders differently.
- b1af369: `keyframes` keys can hold text. A key whose value is a string that isn't path data is accepted wherever its segment eases with `hold`, either the key's own easing or the `keyframes` default when the key has none, so a readout can step between values:

  ```
  =keyframes(frame, 'hold', key(0, 'ten'), key(30, 'hundred'), key(60, '1,024'))
  ```

  Text can't interpolate, so any other easing out of a text key fails with an error that names the key and says text keys need `hold`. Path data keys interpolate as before.

- cfc49e0: Reports the time a render waits for the GPU as `gpu_ms` in the `lumen` summary line, instead of counting it under `readback_ms`. `readback_ms` is now only the copy of finished frames to the CPU, and `flush_ms` only the collection of finished work, which no longer includes the waits that `gpu_ms` holds. The server's `RenderTiming` gains a matching `gpu` field.
- b3a277d: A node whose params read the frame is kept across frames while their values hold. Each frame its expressions are evaluated, along with those of the nodes it reads, and its output from an earlier frame is reused when they give the values it was drawn with. A key track that holds, a `switch` whose `selected_layer` changes once, or an `opacity` that steps from 0 to 1 now redraws only on the frames where a value changes, and so does everything downstream of it. A camera that sits still for 40 frames, moves for 9 and sits still again pays for the 9.

  A shape, path, text or `transform` that blurs its own motion compares its values at either end of its shutter too, and a `motion_blur` over such a node is drawn on every frame, while the samples it takes of its source are reused when their values match. A text node whose `glyph_*` params read the frame, an `instance_3d` whose placement does, video, a `wgsl_shader` and a model playing a clip are drawn on every frame as before.

- cd82e6d: A node whose expressions can't change from frame to frame is kept across frames like one with literal params. `"=vec3(0, 0, 520)"` or `"=max(0.9, 0.5)"` no longer redraws its node, and everything downstream, on every frame. An expression counts as changing when it reads `frame` or `time`, calls `progress` or `tween`, or reads another node, a template prop or measured text, as an `instance_3d` already counted.

  `vec2(..)` and `vec3(..)` of plain numbers are written to a node's params as the literal `[x, y]` or `[x, y, z]`. They are still expressions, so `vec2(1, 2).add(..)` works and `String(vec2(1, 2))` is `"=vec2(1, 2)"`.

  A number written in a composition's JSON is now read exactly, so `[0.9999999999999999, 0]` compares equal to `vec2(0.9999999999999999, 0)` as it does when the vector is written as an expression.

- @lumiscia/lumen-preview@0.6.0-beta.1

## 0.6.0-beta.0

### Minor Changes

- ec34c26: Small 3D API cleanups.

  `image_plane_3d`'s default `width` is 1. Every other 3D primitive defaults to one world unit (a `shape_3d` has `size` `[1, 1, 1]`), so a plane with no params is as wide as a default box beside it. For a 16:9 source about a unit tall, set `"width": 1.6`.

  `image_plane_3d` takes `opaque` (default false). A plane drawn blended, as every plane is by default, is ordered by its centre among the blended objects and doesn't write depth, so it can show in the wrong order against other blended objects it crosses. With `opaque` on, the plane is drawn with the opaque objects and writes depth, so it is ordered by depth against anything it intersects. Use it for a source with no transparent pixels. Alpha is ignored then, the colour's alpha, `opacity` and the source's: a transparent pixel shows its colour at full opacity, which is black for a fully transparent one, and a half-transparent one shows its colour undimmed. `opaque` casts a shadow over the whole rectangle, and has no effect on a `shadow_catcher`, which is always drawn as one.

  ```json
  {
    "id": 2,
    "type": "image_plane_3d",
    "params": { "width": 2, "rotation": [0, 45, 0], "opaque": true }
  }
  ```

  A gradient given as a 3D colour, `color` on `shape_3d`, `image_plane_3d`, `text_3d` or `light_3d`, or `fog_color` on `render_3d`, fails `check` with `node.invalid_param` at that param's path, such as `/nodes/3/params/color`, instead of failing the render. These params' schema type is a color rather than a paint, so `NodeParamKind` has a `"color"` member and the generated TypeScript types of these params don't accept a gradient. No expression can produce a gradient, so nothing but a composition built in Rust rather than parsed from JSON can still reach the render-time "3D colours must be solid" error.

- a146bbd: 3D performance: mipmapped raster sources, and static models once their clip ends.

  An `image_plane_3d` source shown smaller than its pixels, and any `model_3d` texture input larger than 64 pixels a side, is now filtered with mipmaps, trilinearly and anisotropically, instead of being sampled at full size. The mip chains are made on the GPU in the source's own format, averaging each level's texels by area in linear light, and are made once for a source that doesn't change. A source shown at its own size or larger gets none. Minified rasters look smoother, and a render with `supersample` of 1 differs slightly from before where it showed one. A chain adds a third to the raster's memory and a copy of its first level.

  A `model_3d` whose animation doesn't loop is drawn once for every frame after the clip's last key (or before its first, when it holds the first pose), as it is for a model with no animation, so those frames record no 3D passes. This holds for every kind of channel, morph weights and the model's own lights and camera included. A model kept as a static frame is also drawn again when its file changes, as the media resolver reports through the model's id.

- 4bacce6: **Video output with an alpha channel.** Two new output formats hold alpha: `mov`, ProRes 4444 from `prores_ks` (`yuva444p10le`) with AAC audio, and `webm`, VP9 from `libvpx-vp9` (`yuva420p`, tagged `alpha_mode=1`) with Opus audio from `libopus`. A render asks for alpha with the request's `alpha`, `lumen --alpha` or the SDK's `render(..., { alpha: true })`, which win over `render_settings.alpha`, as `codec` does. The default stays opaque, and an `mp4` is unchanged.

  ```json
  { "render_settings": { "width": 1920, "height": 1080, "alpha": true } }
  ```

  ```json
  { "composition": { "...": "..." }, "alpha": true, "output": { "format": "webm" } }
  ```

  The colour is straight, as a PNG of the frame holds it: the premultiplied linear colour is unpremultiplied in linear light, sRGB-encoded and converted to BT.709 limited range once, from float, with the same ordered dither as opaque output. Alpha is full range and never dithered, and a fully transparent pixel is black with neutral chroma. VP9's 4:2:0 chroma is the alpha-weighted average of each 2x2 block, `Σ aᵢ·cᵢ / Σ aᵢ`, so transparent pixels don't darken edges.

  `codec` takes `prores` and `vp9` alongside `h264`, `hevc` and `av1`. A `mov` or `webm` without a codec gets its own; `mp4` holds `h264`, `hevc` and `av1`, `mov` holds `prores` and `webm` holds `vp9`, and anything else is refused with `invalid_output`. ProRes is always 10-bit, VP9 with alpha 8-bit, and either also encodes opaque video. Every built-in preset but `lossless` has `prores_ks` settings (quantiser 8, 4 and 2), and every one has `libvpx-vp9` settings (constant quality, `cpu-used` 0 to 5 as its speed). No hardware encoder writes alpha: auto selection goes straight to the software encoder, and a pinned hardware encoder is refused with `invalid_quality`. Alpha in an `mp4`, or with `h264`, `hevc` or `av1`, is an error, never an opaque video, as is a `prores_ks` option that leaves a profile other than 4444 or, with alpha, an `alpha_bits` other than 16, either of which drops the alpha plane. A composition with audio is refused with `invalid_output` when the render is requested if this FFmpeg has no encoder for the container's audio (`libopus` for WebM, AAC otherwise).

  **Breaking, Rust API:** `lumen_server::render::resolve_encoder` takes the `OutputFormat` after the composition and returns a `ServerRenderError` (`invalid_output` or `invalid_quality`) rather than an `anyhow::Error`. `OutputFormat` has `Mov` and `Webm`, `OutputCodec` and `EncodeCodec` have `ProRes` and `Vp9` (so `EncodeCodec::ALL` has five), and `OutputSettings` has `alpha`. In `lumen-ffmpeg`, `VideoEncoderConfig::alpha`, `VideoEncoderChoice::with_alpha`, `QualityRequest::alpha`, `check_alpha` and `check_audio_encoder` are new, `EncoderFamily` has `Vpx` and `ProRes`, `PixelFormat` has `Yuva420p`, `Yuv444p10` and `Yuva444p10`, and `check_bit_depth` refuses 8-bit ProRes. `lumen_engine::gpu::YuvConverter` also converts to `Yuva420p`, `Yuv444p10` and `Yuva444p10`. Code matching these enums exhaustively needs the new arms.

  An audio encoder now writes the codec it was opened with, where it was always tagged AAC.

- 8ef4ee5: Audio clips fade, volumes animate, and templates carry their own sound.

  - A track's or clip's `volume` takes an `=expression` as well as a number, evaluated in the time of the composition the audio is in: `frame`, `time`, `fps`, `width`, `height` and, inside a template, props. Gains are evaluated every 512 samples and move linearly in between, so they never step. A volume expression that reads a node, a glyph or text, or a prop the template doesn't declare, fails the check with `audio.invalid`; a value below 0 plays as silence.
  - Clips take `fade_in_ms` and `fade_out_ms`, which fade from and to silence at the clip's start and end on an equal-power curve. Fades longer than a short clip multiply where they overlap.
  - A template in `compositions` can have its own `audio`. It plays wherever a `clip` that reaches the output plays the template, at the clip's start, duration, offset, speed and end, as its frames do, through nested clips. Speed resamples, so pitch follows it; past a `hold` window the audio is silent. A `switch` plays only its selected layer's audio, and a `time_remap` plays its source's audio at the remapped time, silent where that time holds or runs backwards.
  - Audio planning is bounded. The root and each template may hold at most 512 audio tracks and 8192 audio clips, and clips that play templates with sound more than 4096 times in all, counting the clips inside the templates they play, fail the check with `audio.invalid`; planning an interval only looks at the clips that overlap it. While rendering or previewing, a 512-sample interval may be inside at most 1024 clips and time remaps (a loop may restart at most 32 times in it) and sound at most 1024 clips, one planning call (the server mixes 30 s at a time) returns at most 500,000 segments, and the preview's plan of the whole timeline covers at most about 3 hours and 200,000 segments; past a limit the render or the preview load fails with the new `audio.limit` code.
  - A prop a clip sets by expression moves a gain smoothly, as the gain's own expression does. Nested audio keeps sounding after a parent loop restarts, including under a `switch` or another loop, and a looping `time_remap` no longer silences the 512 samples around its wrap, whatever its loop bounds.
  - The builder's `addAudio` takes `fadeIn` and `fadeOut` in seconds and `gain` as a number or an `Expr<number>`, `addAudioTrack` takes an `Expr<number>` gain, and `defineComposition` keeps the audio its `build` adds.

  ```json
  {
    "compositions": {
      "whoosh": {
        "timeline": { "fps": 30, "duration_frames": 30 },
        "props": { "level": { "type": "number", "default": 1 } },
        "audio": {
          "tracks": [{ "id": "main" }],
          "clips": [
            {
              "id": "a",
              "source_id": "whoosh.wav",
              "track_id": "main",
              "start_ms": 0,
              "duration_ms": 500,
              "fade_out_ms": 100,
              "volume": "=prop('level')"
            }
          ]
        },
        "nodes": [
          { "id": 1, "type": "background" },
          { "id": 2, "type": "media_output" }
        ],
        "connections": [{ "from_node": 1, "to_node": 2, "to_port": "source" }]
      }
    },
    "audio": {
      "tracks": [{ "id": "music", "volume": "=if(time >= 2 && time < 6, 0.25, 1)" }],
      "clips": [
        {
          "id": "bed",
          "source_id": "music.mp3",
          "track_id": "music",
          "start_ms": 0,
          "duration_ms": 10000,
          "fade_in_ms": 1000,
          "fade_out_ms": 1000
        }
      ]
    }
  }
  ```

  The browser preview plays the same mix: the engine plans it and the preview's audio worker plays the plan.

  Breaking changes:

  - `audioTimeline()` on the preview's wasm controller (`LumenPreviewControllerBinding` in `@lumiscia/lumen-preview/advanced`) returns the engine's audio plan, `{ voices, segments, durationFrames, fps }`, instead of the parsed tracks and clips. The preview's audio engine is internal and takes the plan itself; only a custom host that reads `audioTimeline()` is affected.
  - The generated `AudioClip` and `AudioTrack` types have `volume: ExpressionValue<number>`, and the root `audio` is the new `AudioTimeline` type.
  - In Rust, `AudioMixer::new` takes the `Composition` instead of its timeline, and `mix_range` takes `&mut self` and fails with a `LumenError`. `AudioTrack::volume` and `AudioClip::volume` are an `AudioGain`, and `CompositionTemplate` has an `audio` field. Use `Composition::has_audio` to ask whether a render has sound.

- 97c7016: **Breaking: `color_grade` has one LUT scale, and its inline LUT is the `lut` param.** `color_grade` also reads 3D `.cube` LUTs from files with the new `lut_source` param.

  - The inline 1D LUT, which was `lut_source`, is now `lut` (`"rgb1d:0,0,0;0.2,0.1,0.3;1,1,1"` or `"identity"`). Rename the param in existing compositions.
  - Its scale is 0 to 1. The engine no longer guesses per value whether a number above 1 was meant as 0–255. A value outside 0..1 or not finite fails the render, naming `lut` and the value. **Migration:** divide a 0–255 LUT by 255 (`rgb1d: 0,0,0; 255,128,0` becomes `rgb1d: 0,0,0; 1,0.5,0`).
  - `lut_source` is now a media source, a path, `media` alias or `asset:<id>`, naming an Adobe/Resolve `.cube` file. It reads `TITLE`, `LUT_3D_SIZE`, `DOMAIN_MIN` and `DOMAIN_MAX`, comments and blank lines, and uploads the table as a 3D texture once per media identity. Lookups are trilinear, with the half-texel offset so the end points land on the first and last entries; `interpolation: "nearest"` takes the nearest entry. `strength` blends the graded colour as it does for `lut`. Table values outside 0..1 are kept.
  - The lookup is in the sRGB-encoded domain on straight colour: the colour is unpremultiplied, encoded, clamped to 0..1, looked up, decoded and premultiplied again, and alpha is untouched.
  - A file is read as untrusted input: `LUT_3D_SIZE` up to 129, files up to 64 MiB, and a missing or wrong entry count, a non-finite or out-of-range number, a missing size line, and any other keyword (`LUT_1D_SIZE`, `LUT_3D_INPUT_RANGE`, ...) are errors that say which. 1D `.cube` tables are not supported.
  - Setting both `lut` and `lut_source` is an error. A literal `lut` that is out of range or malformed, both params set, and an inline LUT written in `lut_source` (as old compositions had it) are `node.invalid_param` diagnostics at the param when the composition is checked; an expression-valued param fails at render with the same message.

  **Rust API:** `AssetKind::Lut`, `Asset::Lut`, `MediaResolver::Lut`, the `LutResolver` trait and `MAX_LUT_BYTES` are new, and `#[media(lut)]` declares a param of that kind. A `MediaStore` that matches on `AssetKind` handles the new variant (a store with no LUTs answers `media.not_found`). `lumen_gpu::LayoutKind::Texture3d` binds a filterable 3D texture.

- 228aa26: `lumen-engine`'s renderer has one render call, and the crate's public surface is narrower.

  `GpuCompositionRenderer::render(composition, frame, media, target)` renders and submits a frame. `frame` is any `Into<f64>` in composition frames, so a `u32` frame and a fractional time take the same call. `target` is `RenderTarget::Owned` (a texture of the renderer's) or `RenderTarget::External(ExternalTexture)`. It returns a `RenderedFrame` with the output `raster` (the external texture for an external target) and its `submission`, and `wait(&device)` blocks until the GPU has finished it. Hosts that time the stages apart call `bind_frame`, `upload_frame` and `submit_frame`, which `render` runs in turn. Rendered output, performance and the server's behaviour are unchanged.

  **Breaking, for Rust hosts:**

  | Removed                                                        | Use instead                                                                                                                                            |
  | -------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------ |
  | `render_frame(c, frame, m)`, `render_frame_at(c, time, m)`     | `render(c, frame, m, RenderTarget::Owned)?.raster`                                                                                                     |
  | `render_frame_submitted(c, frame, m)` → `(raster, submission)` | `render(c, frame, m, RenderTarget::Owned)?` → `RenderedFrame { raster, submission }`                                                                   |
  | `render_frame_into_external(c, frame, m, texture)`             | `render(c, frame, m, RenderTarget::External(texture))?`, a `RenderedFrame` in place of `SubmittedExternalTexture` (`.raster.texture`, `.wait(device)`) |
  | `bind_frame(c, frame, m)`, `bind_frame_at(c, time, m)`         | `bind_frame(c, frame, m, target)`                                                                                                                      |
  | `upload_bound_frame(&bound)`                                   | `upload_frame(&bound)`                                                                                                                                 |
  | `submit_render()`                                              | `submit_frame(&bound)`, which refuses a frame recorded before the latest, as uploading does                                                            |
  | `submit_bound_frame(&bound)`                                   | `upload_frame(&bound)` then `submit_frame(&bound)`                                                                                                     |
  - The test-only renderer hooks (`limit_frame_storage`, `recorded_domain`, `recorded_pass_count`, `text_atlas_texture`, `set_full_frame_sources`, `set_single_tile_repeats`, `cached_media_texture_count`, `media_texture_upload_count`) and `BoundFrame`'s `atlas_region_count`, `atlas_region_rects` and `glyph_instances` exist only with the new `test-hooks` cargo feature, which is not a supported host API.
  - `lumen_engine::scope` is no longer public and `lumen_engine::fingerprint` is hidden from the docs (the derive macros name it); `gpu::MediaTextureKey`, `Clip::window`, `ClipWindow::local_time` and `node::source::media_in::map_to_source_frame` are no longer public either.
  - The `lumen_engine::ffmpeg` and `lumen_engine::image` aliases and the `media::ffmpeg` and `media::image` modules are gone: `FfmpegVideoResolver`, `FfmpegAudioResolver`, `FfmpegResolverOptions` and `ImageFileResolver` are at `lumen_engine::media::*`, where they already were too.

  Compositions, the TypeScript SDK and the server's HTTP API are unaffected.

- c4d3140: 3D scenes can be lit by an HDR environment map. Set `environment_map` on `render_3d` to an equirectangular Radiance `.hdr` or OpenEXR `.exr` file, by path or as `asset:<id>`, and it replaces the built-in studio: surfaces take their diffuse light and their reflections from it, rough ones blurred as much as their roughness. `environment` stays its brightness and `environment_rotation` turns it about +Y in degrees. A scene lit by a map and no lights gets no default lights. The map's centre is the view along -Z, with +X to its right and +Y at its top.

  `environment_background` shows the map behind the scene, through the exposure and tone mapping, and makes the render opaque; `environment_background_blur` blurs it from 0 (sharp) to 1. Shadow catchers darken the background by the light their shadows take away:

  ```json
  {
    "nodes": [
      {
        "id": 1,
        "type": "model_3d",
        "params": { "source": "asset:fox", "scale": [0.02, 0.02, 0.02] }
      },
      {
        "id": 2,
        "type": "shape_3d",
        "params": { "shape": "plane", "size": [10, 1, 10], "shadow_catcher": true }
      },
      {
        "id": 3,
        "type": "light_3d",
        "params": {
          "light": "directional",
          "intensity": 2,
          "rotation": [-55, -40, 0],
          "cast_shadows": true
        }
      },
      {
        "id": 4,
        "type": "camera_3d",
        "params": { "position": [0, 1.4, 4.2], "look_at": true, "target": [0, 0.7, 0] }
      },
      { "id": 5, "type": "merge_3d" },
      {
        "id": 6,
        "type": "render_3d",
        "params": {
          "environment_map": "asset:studio",
          "environment_rotation": 90,
          "environment_background": true,
          "environment_background_blur": 0.3
        }
      },
      { "id": 7, "type": "media_output" }
    ],
    "connections": [
      { "from_node": 1, "to_node": 5, "to_port": "objects" },
      { "from_node": 2, "to_node": 5, "to_port": "objects" },
      { "from_node": 3, "to_node": 5, "to_port": "objects" },
      { "from_node": 4, "to_node": 5, "to_port": "objects" },
      { "from_node": 5, "to_node": 6, "to_port": "scene" },
      { "from_node": 6, "to_node": 7, "to_port": "source" }
    ]
  }
  ```

  Each map is read and prefiltered once and kept while renders use it; turning it or changing its brightness doesn't read it again. Files over 256 MiB, maps over 8192 pixels a side or 8192 × 4096 in all, and multi-part, deep or mip-mapped EXR files are refused before they are decoded, and larger maps are shrunk to 4096 × 2048 as they are read. Non-finite and negative pixels read as black, and a map that isn't 2:1 is stretched over the whole sphere. The server accepts `.hdr` and `.exr` uploads as `environment` media with their `width` and `height`.

  Limitations: a map is decoded and prefiltered on the render thread the first time a render uses it, so that frame takes about 0.35 s longer for a 2K or 4K map and about 0.5 s for an 8K one. The server's upload check decodes the whole map too, taking up to about half a second of CPU and 130 MiB at peak for an 8192 × 4096 file.

- c657075: Expressions can write numbers as text with a fixed number of decimals and padding, and join text from parts, so a frame counter, timecode or coordinate readout follows the frame.

  `format(value, decimals?, width?, fill?)` gives `decimals` digits after the point (default 0), then pads on the left with the one-character `fill` (default a space) to `width` characters (default 0). Halves round away from zero on the number as written, so `format(2.5)` is `"3"`, `format(-2.5)` is `"-3"`, `format(0.125, 2)` is `"0.13"` and `format(1.005, 2)` is `"1.01"`; a result that rounds to zero never prints a minus. With a `"0"` fill the sign stays first: `format(-5, 0, 4, "0")` is `"-005"`. `value` must be finite, `decimals` a whole number from 0 to 20 and `width` one from 0 to 64. `concat(a, b, ...)` joins strings and numbers, with numbers shown as a text param shows them (`concat("f ", frame / 7)` reads like `=frame / 7`); vec2, vec3 and colour parts are errors. `+` stays numeric.

  ```json
  {
    "type": "text",
    "params": { "content": "=concat('frame ', format(frame, 0, 4, '0'), ' / 1200')" }
  }
  ```

  ```ts
  c.addNode({
    type: "text",
    params: { content: concat("frame ", format(frame, 0, 4, "0"), " / 1200") },
  });
  ```

  In the TypeScript builder, `format` and `concat` are `Expr<string>`, and a text node's `content` and a span's `text` take `Expr<number>` as well, so `content: round(frame.mul(3.7))` type-checks.

  `format` and `concat` are now reserved names: a composition function or parameter called either needs another name.

- 0b400af: `model_3d` reads compressed glTF models: Draco meshes (`KHR_draco_mesh_compression`), meshopt buffers (`EXT_meshopt_compression`, with its filters and `KHR_mesh_quantization`), and KTX2 textures of Basis Universal ETC1S or UASTC texels (`KHR_texture_basisu`) and WebP textures (`EXT_texture_webp`), which are used in place of a texture's PNG or JPEG fallback when it has one. Files from gltfpack and glTF-Transform load as they are. KTX2 textures are transcoded to RGBA8 on the CPU, using the mip levels they hold. Each kind of compressed data has its own limit on how far a file of its size may expand, and a file past it is rejected before it is decoded. Animated WebP textures are rejected. The Lumen server and `lumen` read all four; the WebAssembly bindings' default build reads meshopt only.
- bba7e08: Images follow their embedded ICC profile, and 16-bit images keep their depth. Until now every image was read as 8-bit sRGB, so a Display P3 photo (as iPhones take) looked dull, an Adobe RGB one washed out, and a 16-bit PNG or TIFF was cut to 8 bits before the float pipeline. Nothing changes in compositions: a tagged or deep image just renders right.

  - **Profiles:** the embedded profile of PNG (`iCCP`), JPEG (`APP2`), WebP, TIFF and GIF is read with `moxcms`. Matrix/TRC RGB profiles (sRGB, Display P3, Adobe RGB, ProPhoto, Rec. 2020 and the like) and grey profiles convert to the working space's linear BT.709, colour outside BT.709 clipped at zero and kept above 1, as video is. An image without a profile is sRGB, as before; PNG `sRGB`, `gAMA` and `cHRM` chunks are not read.
  - **sRGB stays exact:** an 8-bit image with no profile, or one whose profile matches sRGB, decodes and uploads exactly as before, so it still passes through bit for bit.
  - **Wide gamut on the GPU:** an 8-bit image whose profile has the sRGB curve in other primaries, such as Display P3, keeps its 8-bit texture and converts in linear light on the GPU, at no extra memory or time.
  - **Deeper images:** 16-bit PNG and TIFF, float TIFF, and 8-bit images with another curve, such as Adobe RGB, are converted on the CPU to linear half floats, 8 bytes a pixel in the image texture cache (`image_textures` counts them so). A reduced copy is filtered at that precision. Float samples are read as encoded with the image's curve, as `image` reads them; values above 1 keep their light, negative ones clip at zero and ones that aren't numbers are black. The 512 MiB decode limit counts the conversion too, so an image whose half floats don't fit beside its samples fails with `media.too_large` (an 8-bit image still counts only its decoded samples, as before).
  - **Profiles that can't be honoured** (any with A2B lookup tables, even beside a matrix and curves, since a CMM prefers the tables; CMYK; a grey profile on a colour image or the reverse; primaries whose matrix to BT.709 has an element past 8; past 1 MiB; or malformed) read as sRGB, and the render warns once per composition, `media_in` node and source with `media.color_unsupported`, the code untagged video colour already uses, naming why. A profile never fails a render.

  **Breaking, for Rust hosts:**

  - `MediaFrame` has two more variants, `WideRgba { frame, gamut }` and `LinearRgba(Arc<LinearMediaFrame>)`, which `ImageFileResolver` returns for wide-gamut and deep images. A `match` on `MediaFrame` needs arms for them.
  - The warning for colour that can't be honoured is `Warning::UnsupportedColor { kind, media, .. }`, with `kind` `AssetKind::Video` or `AssetKind::Image`, and one code, `media.color_unsupported`, for both.
  - `ImageResolver` has `color_warning()`, `None` by default. A resolver that wraps another should forward it, or the wrapped image's warning is lost.

- 40e1f4e: Images are decoded no larger than they are shown.

  Nodes read an image's texels as its pixels, so an image is reduced only where every use of it is a `resize` with a literal `width`, `height` and `mode`, in the root composition, and only to a size with its aspect ratio exactly, so the resize draws the same picture. Such an image, larger on a side than the device's texture limit, now renders from a reduced copy with a `media.image_reduced` warning (for the first 32 sources per composition) instead of failing as `media.too_large`. `GpuCompositionRenderer::set_max_image_side` sets a lower limit of the host's own. An image used any other way keeps its own size: past `set_max_image_side` it is shown unreduced, and past the device's limit it still fails as `media.too_large`.

  When those resizes show the image at under half its size, it is decoded at twice the size it is shown: a 6000×4000 photo resized to 600×400 is uploaded at 1200×800. What follows the resize may only keep pixels as they are (merges, colour and alpha nodes, crops, blurs, glows, shadows) or scale by a literal `transform` scale. The reduced copy is averaged over each pixel's footprint in linear light, on the CPU, before upload. Image textures are cached by identity and decoded size, so a composition that shows an image larger decodes it again.

  `ImageFileResolver` decodes a file at its full size first, so a file whose decode needs more than the 512 MiB the decoder allows one image still fails as `media.too_large` before any reduction.

  **Breaking, for Rust hosts:** a host that relied on an image past the texture limit failing gets a reduced render and a warning instead, where the image's uses let it be reduced.

- c4d3f90: `image_plane_3d` shows a raster on a flat rectangle in a 3D scene, so text, video, an image or any other 2D node can sit in depth under a real camera. Its `source` input takes the raster, and it outputs a scene like `shape_3d`, with the same `position`, `rotation`, `scale` and material params. A video or image works the same way: wire a `media_in`, or a `resize` of one, into `source`. `width` is in world units and the height follows the source's aspect ratio, taken from its full display size; a `clip` keeps the canvas its content declares, padding or cropping the content to it, rather than the composition's.

  A plane is `unlit` and `double_sided` by default, so one that faces the camera shows its source's colours as in 2D, to within rounding, whatever `render_3d`'s `exposure` and `tone_mapping`. Set `unlit` to false to light it, with the raster as its base colour; `color` multiplies the raster, and `emissive` scales with it. A plane is drawn blended unless `opaque` is on, back to front with other blended objects, so its transparent texels show what is behind it and never hide it, and `opacity` multiplies the source's alpha. Seen from behind, the raster is mirrored. Planes are ordered by their centres, so planes that intersect, or overlap while crossing in depth, can show in the wrong order; layers separated in depth, and opaque shapes in front of or behind planes, are ordered correctly. A source keeps its detail when it is shown at its own size or larger, where `render_3d`'s `supersample` only antialiases the plane's edges; shown smaller, it is filtered with mipmaps before `supersample` averages it.

  ```json
  {
    "nodes": [
      {
        "id": 1,
        "type": "text",
        "params": {
          "content": "Lumen",
          "font_family": "Inter",
          "font_size": 160,
          "position": [420, 280]
        }
      },
      {
        "id": 2,
        "type": "image_plane_3d",
        "params": { "width": 4, "position": [0, 0, -1], "rotation": [0, 25, 0] }
      },
      { "id": 3, "type": "render_3d" },
      { "id": 4, "type": "media_output" }
    ],
    "connections": [
      { "from_node": 1, "to_node": 2, "to_port": "source" },
      { "from_node": 2, "to_node": 3, "to_port": "scene" },
      { "from_node": 3, "to_node": 4, "to_port": "source" }
    ]
  }
  ```

  Wiring a raster into a scene input now fails `connection.wrong_kind` with a message that names `image_plane_3d`.

- dc06fed: `instance_3d` draws its input scene `count` times, from 0 to 100000, sharing the input's meshes, textures, materials and skinned models' joint matrices on the GPU instead of repeating a node for each copy. Each copy is placed by `position`, `rotation` and `scale`, which are evaluated once per copy, and two new expression globals say which copy that is: `instance` is its index from 0 and `instance_count` is `count`, so one node lays out a grid, a ring or a path (`path_point(d, instance / instance_count)`) without separate layout modes. They are only readable in this node's `position`, `rotation` and `scale`, and reading them elsewhere fails like `glyph` does outside a text node's `glyph_*` params. In the TypeScript SDK they are `instance` and `instanceCount`, exported with `frame` and `time`.

  Only the input's objects are copied. Its lights and camera appear once, placed as without the node. Every copy of a skinned model plays the same clip at the same time, and shadows, fog and the environment treat copies like any other object. An `instance_3d` may hold another, up to 8 deep, with their counts multiplied. A scene draws at most 100000 objects in all, counting every copy's objects, and one that would draw more, or more vertices than fit one draw, fails the render naming the node before anything is copied. `count` 0 leaves the scene out entirely. A node whose params don't read `frame` or `time`, or another node, stays static when its input is, so its `render_3d` is kept across frames. The names `instance` and `instance_count` are now reserved, so a composition function can't use them as parameter names.

  ```json
  {
    "nodes": [
      {
        "id": 1,
        "type": "shape_3d",
        "params": { "size": [0.6, 0.6, 0.6], "position": [0, 0.3, 0] }
      },
      {
        "id": 2,
        "type": "instance_3d",
        "params": {
          "count": 100,
          "position": "=vec3(instance % 10 * 1.5 - 6.75, 0, floor(instance / 10) * 1.5 - 6.75)"
        }
      },
      { "id": 3, "type": "render_3d" },
      { "id": 4, "type": "media_output" }
    ],
    "connections": [
      { "from_node": 1, "to_node": 2, "to_port": "scene" },
      { "from_node": 2, "to_node": 3, "to_port": "scene" },
      { "from_node": 3, "to_node": 4, "to_port": "source" }
    ]
  }
  ```

- b577157: **Breaking for hosts that match on `render.gpu`.** The code now means a GPU fault only: the device, its queue or a driver failed, which no composition causes and trying again may fix. It used to cover everything the render raised that no other code fit, so a composition the author had to fix looked like an infrastructure alarm.

  Two new codes take those cases:

  | Code              | Means                                                                                                                                                                                                                            | Was `render.gpu` for                                                                                                                                                                                                                                                                                                                                                            |
  | ----------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
  | `render.limit`    | The composition needs more than the device or engine allows. The author fixes it: less text, smaller media.                                                                                                                      | A frame whose uniform or storage data doesn't fit the device's buffers (for example text with a very large glyph limit), and a CPU video frame larger than the device's texture limit.                                                                                                                                                                                          |
  | `render.internal` | The engine, or a host's resolver or render target, broke a contract the render relies on. A bug to report, not a composition to fix. Trying again helps only for fonts that kept loading, which stay loaded for the next render. | A poisoned text, atlas or font-cache lock, a stale or missing recorded frame passed to `upload_frame` or `submit_frame`, an image resolver returning a video frame, a malformed YUV frame, an external texture of the wrong size, format or usage, fonts that never stopped loading, recorder misuse, and a CUDA or VideoToolbox target asked of a renderer on another backend. |

  A `wgsl_shader` whose source the device refuses (a syntax error, a binding it can't use) is now `render.node_evaluation` at that node, since the source is the composition's.

  The WASM renderer's errors now carry the diagnostic code as `code` for every render failure, not only media ones.

  A video frame that can't be imported on the GPU and then also fails to decode on the CPU now has the media code of the CPU failure (`media.decode`, `media.not_found`, ...) with the same message, instead of `render.gpu`.

  `render.gpu` remains for creating the device and for submitting, validating and reading back work on it. `lumen-server` still treats exactly `render.gpu` (and transient media failures) as retryable, so `render.limit` and `render.internal` are not retried.

  Migration: a host that retried or alerted on `render.gpu` keeps doing so for real GPU faults; one that showed `render.gpu` to authors as "your composition is wrong" should show `render.limit` that way and report `render.internal` as an engine bug. Hosts must already tolerate codes they don't know. The `lumen_gpu::Recorder` now returns a `FrameError` (`TooLarge` or `Misuse`) inside its `anyhow::Error` for these cases.

- d3e4ce9: **Breaking: the compositor works in linear light.** Every raster is now `Rgba16Float`, linear with BT.709 primaries and premultiplied alpha, with 1.0 as SDR white and room above it. JSON colours, images and video are sRGB-encoded and decode on the way in; `media_output` composites over black for opaque targets, encodes back to sRGB and quantises once. An 8-bit image passed straight through comes out byte for byte, and JSON colours render as the codes they name.

  **Pixel output changes** wherever colours mix:

  - Fades, normal merges, masks, blurs, glow, shadows, motion blur, transform and resize sampling, and antialiased edges of shapes and text mix light, not sRGB codes. 50% white over black is code 188, not 128, and a blurred black-white edge is brighter. Small dark text on a light background looks lighter, and light text on dark looks bolder.
  - A luma matte is the linear BT.709 luminance of the mask.
  - `render_3d`'s depth and normal output keeps its values in the working space, so a node reading it reads the value itself, while an 8-bit output encodes them as light: a depth of 0.5 is code 188, not 128.
  - Non-normal blend modes (`multiply`, `screen`, `overlay`, ...), `levels`, `hue_saturation`, the `color_grade` LUT, `exposure`'s contrast and offset, `displacement_map` and `fractal_noise` still work on sRGB-encoded values, so mid grey stays 128 and they look as they did, but what they composite onto is now linear. `exposure` multiplies linear light by 2^stops.
  - 8-bit output is ordered-dithered across the whole code step, so a flat area keeps its value as its mean and smooth gradients lose their banding, except within a tenth of a code of a code, where the output is that code exactly. That keeps 8-bit content and JSON colours bit-exact, dither on or off; a gradient that slow is flat for a tenth of a code at each end of each step. The YUV every video encoder takes is dithered in its final format; `GpuCompositionRenderer::set_output_dither(false)` turns it off.
  - Quantised colour never exceeds quantised alpha, and a pixel whose alpha rounds to 0 is transparent black.
  - Opaque outputs, video and JPEG, composite over black in linear light: half-covered red is code 188, not 128. PNG stills keep straight alpha, quantised from the straight colour.
  - Straight-alpha images are premultiplied on the GPU in linear light, so translucent PNG edges no longer darken. Premultiplied YUV video keeps colour brighter than its alpha.
  - Motion blur sums its samples in 32-bit float and divides once, so a still source comes through unchanged at any sample count. Glow keeps its light above 1 with alpha at most 1. Every raster holds finite values: `exposure`, `levels`, `hue_saturation`, the `color_grade` LUT, merges, glow and shadow clamp light to the largest half float (65504) and make NaN 0, and a `wgsl_shader`'s output has the same done to it, with its alpha clamped to `0..=1`.

  **Migrating a `wgsl_shader`:** inputs are linear and premultiplied, and values above 1 are kept. The output, and any storage texture you declare, must be `texture_storage_2d<rgba16float, write>`; a shader declaring `rgba8unorm` fails with a message saying so. Colour uniforms arrive decoded to linear and premultiplied; numbers and vectors arrive as written. The prelude has `lumen_srgb_to_linear` and `lumen_linear_to_srgb` for formulas that need encoded values:

  ```wgsl
  let color = textureLoad(input0, vec2<i32>(id.xy), 0);
  let encoded = lumen_linear_to_srgb(color.rgb / max(color.a, 1e-6));
  let curved = encoded * encoded * (3.0 - 2.0 * encoded); // a formula written for sRGB codes
  textureStore(output, vec2<i32>(id.xy), vec4<f32>(lumen_srgb_to_linear(curved) * color.a, color.a));
  ```

  **Rust API:** `MediaFrame::RawRgba` holds straight colour, which images now use; `premultiply_rgba_in_place_if_needed` is gone. `VulkanVideoTarget` and `CudaNvencTargetPool` render from `Rgba16Float`. `GpuCompositionRenderer::set_output_alpha` takes an `OutputAlpha`: `Premultiplied` (the default), `Straight`, or `Opaque`, which composites over black for targets without alpha; the server, bench and browser preview set it per output. A float output format takes the working space unchanged, linear and premultiplied.

- 60b7798: **`lumen-local` is now `lumen`, installed from npm.** `npm install -g @lumiscia/lumen-cli` installs a prebuilt `lumen` binary for Linux on x64 and arm64 (glibc 2.28 or later) and for macOS on Apple silicon. FFmpeg 9 is linked into it with `libx264`, `libx265` (8-bit and 10-bit), `libsvtav1`, `libvpx-vp9`, `libopus` and `libdav1d`, plus the Vulkan and NVENC encoders on Linux, where NVENC takes frames through CUDA, and VideoToolbox on macOS, so nothing else needs installing. Those binaries are distributed under GPL-3.0-or-later because of x264 and x265; the source stays Apache-2.0.

  **Breaking:** the crate moves from `crates/local` to `crates/cli` and is named `lumen-cli`, and its binary is `lumen`, so `cargo run -p lumen-local --` becomes `cargo run -p lumen-cli --`. Flags and environment variables are unchanged. Errors now start with `lumen failed`.

  **AV1 uses NVENC on NVIDIA.** On a Linux host with NVIDIA's encode library, `av1` now tries `av1_nvenc`, then `libsvtav1`, instead of `av1_vulkan`, which writes wrong frames on NVIDIA. Other hosts and codecs keep their order.

  **Fixed:** `hevc_nvenc` failed to open with a `crf` or a built-in preset, because `maxrate` was set as an option of the encoder and is one of the codec context. `lumen-engine-ffmpeg` also builds against FFmpeg 9 now, which removed `AVCodec::sample_fmts`.

- 716f9d0: One place to set and read the engine's media cache budgets.

  `GpuCompositionRenderer::set_cache_budgets(CacheBudgets)` sets the byte budget of each cache the renderer keeps media and derived GPU data in: image textures (256 MiB), pooled textures (384 MiB), retained outputs (1 GiB), `.cube` LUTs (128 MiB), mip chains (256 MiB), 3D models (256 MiB), environment maps (256 MiB) and `text_3d` meshes (64 MiB). The defaults are unchanged. `cache_budgets()` reads them back, and `cache_stats()` returns a `CacheStats` with the bytes each cache holds now, plus the glyph atlas. What a frame uses, and what a kept output was drawn from, is never freed for a budget. Only the image texture budget counts just idle textures. The others count each cache's total, so entries in use can push a cache past its budget and free all its idle ones. The `models` figure includes the CPU bounds kept beside each model's GPU buffers and textures, and the pool frees its spares by size bucket rather than strictly least recently used first.

  **Breaking, for Rust hosts:** `set_media_texture_cache_budget(bytes)` and `DEFAULT_MEDIA_TEXTURE_CACHE_BUDGET_BYTES` are removed, along with the hidden `set_frame_graph_pool_budget` and `set_frame_graph_retained_budget`. Replace `renderer.set_media_texture_cache_budget(bytes)` with `renderer.set_cache_budgets(CacheBudgets { image_textures: bytes, ..renderer.cache_budgets() })`, and the others with the `pooled_textures` and `retained_outputs` fields in the same way. Compositions, the server and the TypeScript SDK are unaffected.

- 0aead67: **Breaking (Rust API): a host supplies every kind of media through one `MediaStore::resolve`, and every resolved source carries a `MediaIdentity`.** Compositions, renders and the browser bindings' JavaScript API are unchanged; this breaks every Rust embedder that implements or calls `MediaStore`.

  `MediaStore` has one required method, `resolve(&self, source: &str, kind: AssetKind) -> Result<ResolvedMedia, MediaError>`, plus `text_session` as before. `ResolvedMedia { identity, resolver }` holds a `MediaResolver`, an enum with one variant per kind wrapping that kind's existing resolver trait object. `media::resolve_as::<dyn ImageResolver>(store, source)` resolves and returns `(MediaIdentity, Box<dyn ImageResolver>)`; a store that serves another kind than asked fails there as `media.wrong_kind` with `wanted` and `found` set, so a store that holds one kind per name can return whatever it has.

  `MediaIdentity` replaces each resolver's `id()`, with one rule: the engine treats equal identities, from any store in the process, as the same bytes, so a host gives a source a new one when its bytes change. Every cache that outlives a frame keys on it: image textures and their mip chains, outputs kept across frames (checked against the identity of every image, model, environment map and font family they read before reuse), loaded fonts, GPU model buffers, environment maps and the audio mixer's decoded blocks. Build one with `MediaIdentity::of_key(key)` for a content hash, version counter or immutable upload's id, `unique()` for bytes handed over in memory, taken again on each set, or `of_file(path)` / `of_files(paths)`, a heuristic from the file's metadata (canonical path, length, modification time and, on Unix, device and inode) that misses a same-length write in place within the modification time's resolution; hosts that know better should use `of_key`. `ImageFileResolver` and `FfmpegVideoResolver` named themselves by their path, so a renderer kept across stores could draw a replaced image's old pixels; lumen-server now names files under the media root with `of_file` for every kind, and `asset:` sources by their asset id, as an asset's content never changes under its id.

  A font family the store doesn't have is now `Err(media.not_found)` rather than `Ok(None)`; the engine falls back to other fonts for it unless the family is an `asset:` reference, which fails as before. Video decode options stay on the store, as `LocalMediaStore` already kept them.

  | Before                                                                                                            | After                                                                                  |
  | ----------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------- |
  | `fn get_image_resolver(&self, source)`                                                                            | `fn resolve(&self, source, AssetKind::Image)` returning `MediaResolver::Image`         |
  | `fn get_video_resolver(&self, stream_id)`                                                                         | `resolve(source, AssetKind::Video)` returning `MediaResolver::Video`                   |
  | `fn get_audio_resolver(&self, source)`                                                                            | `resolve(source, AssetKind::Audio)` returning `MediaResolver::Audio`                   |
  | `fn get_model_resolver(&self, source)`                                                                            | `resolve(source, AssetKind::Model)` returning `MediaResolver::Model`                   |
  | `fn get_environment_resolver(&self, source)`                                                                      | `resolve(source, AssetKind::Environment)` returning `MediaResolver::Environment`       |
  | `fn get_font_resolver(&self, family) -> Ok(Some(..))`                                                             | `resolve(family, AssetKind::Font)` returning `MediaResolver::Font`                     |
  | `fn get_font_resolver(&self, family) -> Ok(None)`                                                                 | `Err(MediaError::source_not_found(family, ..))`                                        |
  | `fn font_resolver_id(&self, family)`                                                                              | `resolve(family, AssetKind::Font)?.identity`, so fonts need their bytes read lazily    |
  | `ImageResolver::id`, `VideoFrameResolver::id`, `FontResolver::id`, `ModelResolver::id`, `EnvironmentResolver::id` | removed; return the identity in `ResolvedMedia`                                        |
  | calling `store.get_image_resolver(source)`                                                                        | `media::resolve_as::<dyn ImageResolver>(store, source)`, likewise for each kind        |
  | a server or file id string such as `format!("{path}:{len}:{mtime:?}")`                                            | `MediaIdentity::of_file(path)`, a metadata heuristic, or `of_key` when bytes are known |
  | a version counter per registration                                                                                | `MediaIdentity::unique()` on each set                                                  |

- 89568da: A node param is declared as a media source in one place, and a composition can list what it needs before it renders.

  A param struct marks a `String` param with `#[media(image, video)]`, `#[media(model)]` or `#[media(environment)]`, and `by = kind` names the enum param that picks among several kinds. `media_in.source`, `model_3d.source`, `render_3d.environment_map` and an audio clip's `source_id` are declared this way. Asset validation, the schema and the browser preview now read the declaration instead of each knowing which nodes take sources.

  `Composition::media_dependencies()` returns the distinct sources a composition's nodes and audio clips name as written, with the kinds each use reads and where it is used (node and param, or audio clip, inside a template too). A source an expression computes is listed as dynamic, and the font families text uses are listed apart from media. The browser bindings expose it as `mediaDependencies(compositionJson)`, typed as `MediaInventory` in `@lumiscia/lumen-types`.

  The generated schema gives each such param `x-lumen-media` (`{ "kinds": ["image", "video"], "selectedBy": "kind" }`), and `@lumiscia/lumen-types` exports the generated `nodeMediaSources` table (node kind → param → kinds) with `MediaSourceParam` and `MediaSourceKind`. The preview reads it to tell what a `media` entry holds.

  **Breaking for Rust:** `PropertyDef` has a `media` field, and the `Node` and `NodeParams` traits have a required `for_each_media_source`; hand-written implementations must add it (the `Node` and `Delegate` derives do). `lumen_engine::media::resolve_asset_of` accepts any of several kinds. `LumenPreviewBindings` has a required `mediaDependencies`, so a bindings object built by hand must add it. Compositions change in one way only: a name in `compositions` may be at most 256 bytes long; a longer one fails with `document.invalid`.

- b1f3863: `transform` and `resize` filter a source they show smaller than its size, so a 4K image or video shrunk to a thumbnail, or a zoom-out animation, no longer aliases and shimmers. Each output pixel averages the premultiplied linear light of the source texels it covers, weighted by area, along each of the source's axes, so shrinking one axis leaves the other as sharp as it was. The filter is bilinear at a scale of 1 and widens smoothly from there, and it takes the box on the source itself down to a quarter of its size, and past that on its mip levels, blending the two around the one the scale asks for, so an animated zoom changes smoothly. A source shown between a quarter and its full size builds no mip chain. The mip chain is built once for a source that stays the same from frame to frame and each frame for one that changes, and a source that several nodes shrink shares one. Each tap of a motion-blurred `transform` is filtered for its own scale. `sampling: "linear"` does this; `nearest` takes the nearest texel as before. Magnification and scales of 1 or more are unchanged pixel for pixel, rotation at a scale of 1 included. The mip chains the engine keeps, which 3D scenes also use, have a byte budget, `CacheBudgets::mip_chains` (256 MiB), and `CacheStats::mip_chains` reports what they hold. There is no new param.
- 16aae16: `model_3d` draws more of glTF:

  - **Vertex colours** (`COLOR_0`, RGB or RGBA, float or normalised 8- or 16-bit) multiply the base colour in linear light, as its factor and texture do, lit or unlit, and their alpha blends or masks with it.
  - **A second set of texture coordinates** (`TEXCOORD_1`): each map reads the set its `texCoord` names. A map naming a third set is left out.
  - **`KHR_texture_transform`** on every map: offset, rotation and scale, and its own `texCoord`, which overrides the map's. Rotation turns as the Khronos sample renderer and three.js do.
  - **Morph targets**: position and normal displacements, blended on the GPU by the mesh's or node's default weights and by clips animating `weights`, before skinning. Tangent displacements are ignored. A mesh may have up to 256 targets, and a model up to 4,000,000 displacements (one for each vertex of each target) and 1,048,576 weights; a scene draws up to 1,048,576 weights in a frame. A weight may be at most 10,000 either way (a clip sample overshooting it is held to it), and a vertex its targets would move past what a float holds at that weight is rejected. Shadows hold a morphed mesh at whatever weights it is drawn with. Morphed normals are unit length before skinning, falling back to the unmorphed normal where targets cancel it, and every vertex normal is normalised before it is interpolated. Draco-compressed primitives with morph targets are rejected. A model with still weights is kept across frames like any still model, and only the weights are uploaded each frame while a clip plays them. A clip's `duration` counts its weight channels.
  - **The model's own lights and cameras**, opt-in. `lights: true` adds its `KHR_lights_punctual` lights where its nodes place them. Their units are `light_3d`'s: a directional light's lux and a point or spot light's candela are used as they are, so a point light of intensity `I` lights a surface `d` metres away as a directional light of `I / d²` does, and a spot light fades from its inner cone to its outer one. A light whose colour times intensity overflows is rejected. They cast no shadows. `camera` views the scene from one of the model's cameras, by name or else index, placed by the first node that places it without that node's scale; it counts as a `camera_3d` at the model, so the first camera in the scene still wins, and the output's aspect ratio is used. `modelInfo` and an uploaded model's `metadata` list the cameras (`index`, and `name` when named).

  Points, lines, and triangle strips and fans are still not drawn, and now raise a `scene.model_primitives_skipped` warning naming the model and how many were left out.

  Nothing in a composition breaks, but models now look as their files describe: vertex colours tint them, maps naming `TEXCOORD_1` read it instead of the first set, and morph targets move them. Files requiring `KHR_texture_transform` or `KHR_lights_punctual` are no longer rejected. `LumenModelInfo` gains `cameras`, and the Rust `ModelInfo` a `cameras` field.

  ```json
  {
    "id": 1,
    "type": "model_3d",
    "params": { "source": "asset:robot", "animation": "Talk", "lights": true, "camera": "Closeup" }
  }
  ```

  ```ts
  import { createLumenBindings } from "@lumiscia/lumen-bindings";
  import { nodes } from "@lumiscia/lumen-shared";

  const robot = nodes.model_3d({
    source: "asset:robot",
    animation: "Talk",
    lights: true,
    camera: "Closeup",
  });
  const { modelInfo } = await createLumenBindings().preview();
  const { cameras } = modelInfo(bytes); // [{ index: 0, name: "Closeup" }]
  ```

- 6528c7b: The browser preview draws `model_3d`. Pass each model's `.glb` file in `media`, under the name its `source` uses, as you would an image:

  ```ts
  const fox = composition.addNode({
    type: "model_3d",
    params: { source: "fox", animation: "Walk", scale: [0.01, 0.01, 0.01] },
  });
  // ...wire `fox` into a merge_3d or render_3d as usual.

  mountPreview(host, {
    preview,
    composition,
    media: { fox: "/models/fox.glb" },
    wasmUrl,
    workerUrl,
  });
  ```

  A name that a `model_3d` uses is read as a model, and so is a blob of type `model/gltf-binary`; a source that only an expression names needs `{ kind: "model", source }` when neither its name nor its blob's type says. An `asset:<id>` source is a `media` name like any other. A `model_3d` or `media_in` inside a `compositions` template now says what its `media` entry holds just as one at the root does, so an image a template reads no longer needs a `kind` either.

  Models load with the composition, as images do: the preview finishes loading, and draws its first frame, once every model has arrived, and until then keeps showing what it showed before. A model that can't be fetched fails the load with `load_failed`, naming the source. A file that isn't a model Lumen can read fails the load with the `media.*` code a render would give, before anything is drawn. A model left out of `media` is released, along with its GPU copy.

  The preview now parses the composition before it fetches any media, so a composition the engine rejects fails at once instead of after its media has loaded. A preview built without 3D support reports `node.unsupported` for a `model_3d` without fetching its file.

- b9cc4f1: `model_3d` puts a glTF model in a 3D scene. Its `source` is a binary glTF (`.glb`) file, a path or `asset:<id>`, with its buffers and images packed inside it. It draws the model's default scene with its materials, and plays one of its animation clips: `animation` names the clip, by its name or its index, and empty plays none. `animation_speed` multiplies the clip's speed, `animation_offset` starts it that many seconds in, and `animation_loop`, on by default, plays it again from its start once it ends; off, the model holds the clip's last pose. `position`, `rotation` and `scale` place the model like any other scene node. A clip or material the model lacks fails the render with a message listing the ones it has. A model that plays no clip, or plays one at speed 0, is drawn once and kept across frames, like a still shape.

  ```json
  {
    "nodes": [
      {
        "id": 1,
        "type": "model_3d",
        "params": {
          "source": "asset:fox",
          "animation": "Walk",
          "scale": [0.02, 0.02, 0.02],
          "rotation": [0, 90, 0]
        }
      },
      { "id": 2, "type": "render_3d" },
      { "id": 3, "type": "media_output" }
    ],
    "connections": [
      { "from_node": 1, "to_node": 2, "to_port": "scene" },
      { "from_node": 2, "to_node": 3, "to_port": "source" }
    ]
  }
  ```

  A raster wired into `texture` replaces the material `texture_material` names, shown unlit with its colours as in 2D and sampled as `image_plane_3d` samples it. That puts a video on a television's screen:

  ```json
  {
    "nodes": [
      { "id": 1, "type": "media_in", "params": { "kind": "video", "source": "asset:trailer" } },
      {
        "id": 2,
        "type": "model_3d",
        "params": { "source": "asset:television", "texture_material": "screen" }
      },
      { "id": 3, "type": "render_3d" },
      { "id": 4, "type": "media_output" }
    ],
    "connections": [
      { "from_node": 1, "to_node": 2, "to_port": "texture" },
      { "from_node": 2, "to_node": 3, "to_port": "scene" },
      { "from_node": 3, "to_node": 4, "to_port": "source" }
    ]
  }
  ```

  What is read from the model:

  - The default scene's node hierarchy, with each node's matrix or translation, rotation and scale.
  - Triangle meshes: positions, normals (faces without them are lit flat), the first set of texture coordinates, and indices. Quantized attributes (`KHR_mesh_quantization`) are read too.
  - Metallic-roughness materials: base colour, metallic-roughness, normal, emissive and occlusion textures, with the normal scale and occlusion strength; opaque, mask and blend alpha modes; double-sided materials; `KHR_materials_unlit` and `KHR_materials_emissive_strength`.
  - PNG and JPEG textures, with their samplers' wrap and filter modes.
  - Skins, and animation clips of node translation, rotation and scale with step, linear and cubic-spline interpolation.

  What isn't:

  - Files outside the `.glb`: a model that refers to one is rejected.
  - glTF extensions this release doesn't read, such as `KHR_materials_clearcoat`. A model that requires one is rejected naming it; one that only uses it optionally loads without it.
  - Points, lines, and triangle strips and fans. A model with them loads and draws without them, with a `scene.model_primitives_skipped` warning.

  The Lumen server takes `.glb` uploads as `model` assets, checking each by reading it as renders do.

- a37b057: A model's animation clips and materials can be listed, so `animation` and `texture_material` no longer have to be guessed from an error.

  An uploaded model's media object has `kind: "model"` and a `metadata` listing them, read once when the upload completes and returned by `GET /media/:id`:

  ```json
  {
    "kind": "model",
    "metadata": {
      "animations": [
        { "index": 0, "name": "Walk", "duration": 0.7 },
        { "index": 1, "duration": 2 }
      ],
      "materials": [{ "index": 0, "name": "screen", "hasBaseColorTexture": false }],
      "cameras": []
    }
  }
  ```

  In the browser, `modelInfo` on the bindings reads a `.glb` and returns the same lists, throwing on a file a render could not read:

  ```ts
  const { modelInfo } = await createLumenBindings().preview();
  const { animations, materials } = modelInfo(new Uint8Array(await file.arrayBuffer()));
  ```

  `TemporaryMedia` has `kind` and `metadata` (`MediaKind`, `MediaMetadata`), `@lumiscia/lumen-types` has `LumenModelInfo`, and `uploadMedia` knows `.glb` file names. The error for an `animation` or `texture_material` the model lacks now lists at most 20 names, then how many more there are.

- 6487216: `@lumiscia/lumen-preview` exports only what an application embedding a preview needs. The media bridge, the audio engine and the controller plumbing are no longer public, and the bindings contract moves to a new `@lumiscia/lumen-preview/advanced` subpath.

  The root entry now exports:

  - `createLumenPreview`, `LumenPreviewContext`, `mountPreview`, `LumenPreviewSession`
  - the state and error types: `LumenPreviewState`, `LumenPreviewError`, `LumenPreviewErrorCode`, `LumenPreviewListener`, `LumenPreviewMediaState`, `LumenPreviewPatch`, `LumenPreviewTransport`
  - the option and input types: `LumenPreviewSessionOptions`, `LumenPreviewSessionInputs`, `LumenPreviewComposition`, `LumenPreviewMedia`, `LumenPreviewMediaEntry`, `LumenPreviewMediaKind`, `LumenPreviewFonts`, `LumenPreviewRange`, `LumenLogLevel`, `LumenAudioEngineOptions`
  - the callback types: `LumenPreviewStats`, `LumenPreviewStatsCallback`, `LumenPreviewTimeUpdate`, `LumenPreviewTimeUpdateCallback`
  - `MediaFailure`, the type of `LumenPreviewError.media`, re-exported from `@lumiscia/lumen-types`
  - the source types a `media` or `fonts` entry takes: `MediaSourceInput`, `BlobMediaSourceInput`, `BufferMediaSourceInput`, `UrlMediaSourceInput`

  `@lumiscia/lumen-preview/advanced` exports only types, the contract of a custom host built on `@lumiscia/lumen-preview/worker-host`: the bindings it supplies (`LumenBindings`, `LumenPreviewBindings`, `LumenPreviewControllerBinding`, `LumenMediaTarget`) and the types the worker's messages carry (`VideoSourceRegistration`, `PreviewClock`). The worker's `load` message carries `[source, entry]` pairs of the page's `media`, and the worker works out what to load. `@lumiscia/lumen-bindings` implements the bindings. Every type that appears in a signature of the root, `/advanced`, `./preview` or `./worker-host` can be imported from the root or `/advanced`, apart from types of `@lumiscia/lumen-types` and `mediabunny`.

  `LumenPreviewMediaEntry` now lists its `fps`, `formats` and `track` fields itself instead of extending `VideoSourceRegistration`; the shape is the same.

  Removed from the root entry:

  | Removed                                                                                                                                                                                                                                                                                                                                               | Where it went                      |
  | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------- |
  | `LumenAudioEngine`, `defaultAudioWorkerUrl`, `defaultAudioWorkletUrl`, `AudioSourceRegistration`                                                                                                                                                                                                                                                      | Internal                           |
  | `createLumenPreviewRuntime`, `LumenPreviewRuntime`, `LumenPreviewController`, `resolveLumenPreviewBindings`, `LumenPreviewBindingSource` (and the controller's `registerImageSource`, `registerModelSource`, `registerVideoSource`, `registerVideoSourceBytes`, `loadVideoFrame`, `loadFrameRequirements` and `renderNowAsync`, which nothing called) | Removed                            |
  | `ImageSourceRegistration`, `VideoFrameMetadata`, `VideoFramePayload`, `VideoFramePixels`, `DecodedImageFrame`, `FrameRequirementsPayload`, `FrameRequirementsVideoPayload`, `LumenMediaBridgeOptions`, `NativeVideoFrameTarget`, `MediaRegistration`, `RegisteredImageSource`, `RegisteredModelSource`, `RegisteredVideoSource`                       | Internal                           |
  | `VideoSourceRegistration`, `LumenMediaTarget`                                                                                                                                                                                                                                                                                                         | `@lumiscia/lumen-preview/advanced` |
  | `LumenBindings`, `LumenPreviewBindings`, `LumenPreviewControllerBinding`                                                                                                                                                                                                                                                                              | `@lumiscia/lumen-preview/advanced` |

  The `./audio-engine`, `./media` and `./session` subpaths are removed too, and so is everything they exported (`LumenMediaBridge`, `LumenMediaError`, `toUint8Array`, `createVideoTimeline` and the rest of the media bridge). `./preview` and `./worker-host` stay as they were. `@lumiscia/lumen-react` and `@lumiscia/lumen-svelte` no longer re-export the `MediaRegistration` type.

  Migrating:

  | Before                                                                                 | After                                                                                         |
  | -------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------- |
  | `import type { LumenBindings } from "@lumiscia/lumen-preview"`                         | `import type { LumenBindings } from "@lumiscia/lumen-preview/advanced"`                       |
  | `import { LumenMediaBridge, … } from "@lumiscia/lumen-preview"` or `…/media`           | No replacement. Pass `media` and `fonts` to `mountPreview` or `Player`.                       |
  | `LumenAudioEngine`, `defaultAudioWorkerUrl()`                                          | No replacement. Pass `audio.workerModuleUrl` to `mountPreview` to serve the worker elsewhere. |
  | `createLumenPreviewRuntime(bindings)` and the controller types                         | No replacement. Mount with `mountPreview`, or host the worker with `…/worker-host`.           |
  | `import … from "@lumiscia/lumen-preview/session"`                                      | `import … from "@lumiscia/lumen-preview"`                                                     |
  | `MediaRegistration` from the root, `@lumiscia/lumen-react` or `@lumiscia/lumen-svelte` | No replacement. Pass the page's `media` entries as they are.                                  |

- 1879afe: Every media source that can't be used fails the same way, for images, videos, audio, fonts, models and environment maps, in the engine, `check`, `lumen-server`, `lumen` and the preview.

  A failure names the source as the composition wrote it, the kinds the place using it reads, the kind it is when that is established (a container such as MP4 or WebM can't be told as video or audio from its header, so it has no `found` until it is opened), where it is used (node and param, audio clip, template), and why, with one of five public codes:

  | Code                | Means                                                                                                                                       |
  | ------------------- | ------------------------------------------------------------------------------------------------------------------------------------------- |
  | `media.not_found`   | Nothing is at the source: no file, no URL answer, no asset with that id                                                                     |
  | `media.wrong_kind`  | The source is another kind of media than the use reads, such as a video in an `image` node or a font that is a PNG                          |
  | `media.unsupported` | The engine identifies the data (a container, codec, glTF extension, `.gltf` JSON, WOFF) and doesn't read it                                 |
  | `media.too_large`   | Over a limit (file bytes, pixels, the device's texture size, a model's counts), with the `limit` and the `actual` value when they are known |
  | `media.decode`      | The data is corrupt, truncated, contradicts itself, or is in no format the engine identifies                                                |

  `media.decode` and `media.frame_out_of_range` keep their codes. `check` reports missing and wrong-kind `asset:` references with these codes and the JSON pointer of the param or audio clip, also inside templates. `lumen-server` carries the engine's code through unchanged, and marks a render retryable only for a shortage that can pass (out of memory or file descriptors, `EAGAIN`, a busy device), in the engine's reads, in FFmpeg's open and decode, and while resolving a path. Failures after a source opened name it as the composition wrote it, and the HTTP error message never shows a path on the server. A `.gltf` JSON file is `media.unsupported` in renders and in the upload probe; other text that starts with `{` is `media.decode`. The upload probe's rejection reasons end with the code, such as `(media.too_large)`. `lumen` prints `lumen failed (media.not_found): ...`.

  The thrown error of the WASM bindings carries `code` (the dotted code for a media failure) and `media` (the structured fields). `LumenPreviewError` has a new `media` field with them, typed as `MediaFailure` in `@lumiscia/lumen-types`, and it is `null` for other failures. The preview's own reasons agree with the engine's: an audio-only file registered as video is `media.wrong_kind` (found `audio`), a video the browser can't decode is `media.unsupported`, and a file it can't open or identify is `media.decode`.

  Migrating:

  | Before                                                                         | After                                                                                           |
  | ------------------------------------------------------------------------------ | ----------------------------------------------------------------------------------------------- |
  | `asset.not_found` (check, render)                                              | `media.not_found`, with the source `asset:<id>`                                                 |
  | `asset.wrong_kind` (check, render)                                             | `media.wrong_kind`                                                                              |
  | `media.not_found` for a missing file, URL, font or model                       | `media.not_found` (unchanged), now with the kinds wanted and where it is used                   |
  | `media.decode` for a file of another kind, an unsupported codec or a size cap  | `media.wrong_kind`, `media.unsupported` or `media.too_large`; corrupt data stays `media.decode` |
  | Rust `MediaError::{SourceNotFound, Decode, AssetNotFound, AssetWrongKind}`     | `MediaError::Failed(MediaFailure)`; build one with `MediaError::source_not_found` or `decode`   |
  | The 3D crate's `ModelError::TooLarge(String)` and `EnvironmentError::TooLarge` | `TooLarge { details, limit, actual }`                                                           |
  | Messages such as ``failed to decode image `a.png`: ...``                       | ``image `a.png` (node 3 `source`) failed to decode: ...``; match the `code`, not the message    |
  | An upload probe's rejection reason                                             | The same reason followed by its code, such as `... (media.decode)`                              |

  ```json
  {
    "severity": "error",
    "code": "media.wrong_kind",
    "path": "/nodes/4/params/source",
    "node": 5,
    "message": "`asset:clip` (node 5 `source`) is a video, not an image"
  }
  ```

- 5ab096d: Frame ranges, stills and image sequences, with one output description shared by `lumen`, the `lumen-server` library and its HTTP API.

  **Breaking (Rust, `lumen-server` library):** `render_project_mp4` is now `render_project`, which returns a `RenderedOutput { bytes, format }` instead of the MP4 bytes, and `Mp4Render::open` takes a `&RenderOutput` after the output path. `RenderOptions` gains `output` (a `RenderOutput`) and `sequence_limits`; the defaults, an MP4 of the whole timeline, render as before. Migrate with `render_project(..)?.bytes`, and pass `&RenderOutput::default()` to `Mp4Render::open`. The new `ImageRender` renders stills and sequences, and `ErrorCode` has `InvalidOutput` (`invalid_output`) and `OutputTooLarge` (`output_too_large`).

  ```rust
  let options = RenderOptions {
      output: RenderOutput {
          format: OutputFormat::PngSequence, // Mp4, Mov, Webm, Png, Jpeg, PngSequence or JpegSequence
          frames: FrameRange { start: Some(30), end: Some(60) }, // end not included
          ..RenderOutput::default()
      },
      ..RenderOptions::default()
  };
  ```

  `lumen` takes `--frames START..END`, `.jpg` outputs with `--jpeg-quality`, and an output name with `%d` (`frames/shot_%d.png`) for an image per frame. An MP4 of a range starts at time 0 with its audio cut to the same span.

  `POST /renders` takes an optional `output`: `{ "format": "png_sequence", "frames": { "start": 0, "end": 120 } }`. `format` is `mp4` (the default), `mov`, `webm`, `png`, `jpeg`, `png_sequence` or `jpeg_sequence`; `jpeg_quality` is 1 to 100. The artifact has the format's `Content-Type` and filename (`.mp4`, `.mov`, `.webm`, `.png`, `.jpg`, or a stored `.zip` of `frame_0000.png` files), and `quality`, `codec`, `bit_depth` and `alpha` apply to the video formats (`mp4`, `mov`, `webm`) only. Sequences are bounded by the new `--max-sequence-frames` (3600) and `--max-sequence-bytes` (512 MiB). Requests without `output` behave as before. The SDK's `render` takes `output` and exports `outputFile(output)`.

- d247928: The preview converts colour as a render does. Images are decoded with their embedded colour profile honoured and converted to sRGB explicitly (`createImageBitmap` with `colorSpaceConversion: "default"`, read back through an sRGB 2D canvas), whatever the display, so a Display P3 photo looks as it does in a render within 8-bit rounding. Video frames handed to the engine as YUV planes now carry the source's tagged primaries and transfer, so BT.2020 and Display P3 video is converted to BT.709 and PQ and HLG video is tone-mapped to SDR as a render does, instead of being read as SDR BT.709. A YUV tag the engine doesn't convert, such as a `linear` transfer or unknown primaries, warns once per source with `media.color_unsupported`, as a render does. Video frames the browser hands over as RGB in other primaries or another transfer are drawn on an sRGB canvas, which converts them; sRGB, BT.709 and BT.601 ones are copied as they are.

  The preview is still not pixel-identical to a render. 16-bit images arrive in the browser at 8 bits, and colour outside sRGB is clipped there. Images pass through an 8-bit canvas that stores colour premultiplied, so straight colour at very low alpha is coarsely rounded, as before. Browsers' decoders and colour management differ, by a code or so.

  `setVideoFrameYuv` on the preview's wasm controller takes two more optional arguments at the end, `primaries` and `transfer`, the source's WebCodecs tags; a host calling it directly that leaves them out gets BT.709 SDR as before.

- d63ba1d: The browser preview loads `.cube` LUTs, and a failed render tells the host the engine's code for it.

  - A `color_grade` with `lut_source` works in the preview. Put the `.cube` file in `media` under the name `lut_source` holds, and it loads like a model or an environment map: `pending`, then `ready` or `failed`, counted in `state.media`, awaited by `whenReady()`, fetched abortably and released when the composition stops naming it. It is read in full when it is registered, so a file that isn't a table the engine can use fails the load with the `media.decode`, `media.unsupported` or `media.too_large` a render would give.
  - `LumenPreviewMediaKind` gains `"lut"`. An entry whose source is a `File` named `*.cube`, or a string, `URL`, `Request` or `{ kind: "url" }` source whose path ends in `.cube`, is read as a LUT without a `kind`.
  - The wasm controller gains `setLut`, `hasLut` and `removeLutSource`, and `LumenMediaTarget` gains them as optional members. `setLut` throws, with `media`, on a file it can't read.
  - **A `render_failed` error now carries the engine's code.** Its `diagnostics` hold one `LumenDiagnostic` with that code (`render.gpu`, `render.limit`, `render.node_evaluation` and the others the engine lists), severity `error`, an empty `path` and the message, so `onError` can tell a composition that needs fixing from a GPU fault. It was always empty for a render failure. A render that failed on a media source keeps `diagnostics` empty and says so in `media`. The `render-failed` worker event gains a required `diagnostics` array.

- 7a87028: The browser preview loads every kind of media one way. What the engine lists the composition reading, joined with the page's `media` and `fonts`, decides what is fetched: images, videos, audio, models, environment maps and fonts each go `pending`, then `ready` or `failed`, and are released when the composition stops naming them.

  - `session.whenReady()` and `preview.whenReady()` resolve once a frame of the current composition is drawn and nothing it needs is pending, with the error that stopped it loading, the `render_failed` error of a first frame that couldn't be drawn, an `audio_failed` error, or `null`. They never reject, so check the value; a composition, `media` or `fonts` change starts them again.
  - A model or environment map is read in full before it counts as ready, so a file that isn't one fails the load with the `media.*` code a render would give. A video is ready once its stream is opened; its frames decode as playback reaches them.
  - `state.media` counts the composition's sources as `{ pending, ready, failed }` (the new `LumenPreviewMediaState` type).
  - While anything is pending the preview holds its last frame, at its size, and playback holds too: the playhead stops and the audio pauses until everything has settled. Playback also waits when the frame it is on has spent 100 ms decoding its video frames, so sound can't run ahead of the picture. A load fails as soon as one source fails, and cancels what was still loading for it.
  - Environment maps (`render_3d.environment_map`, Radiance `.hdr` or OpenEXR `.exr`) load from `media` like models. `LumenPreviewMediaKind` gains `"environment"`.
  - Source names follow `lumen-server`'s rules, checked against a fixture both test suites read. The `@lumiscia/lumen-preview` README has the table.
  - `runtimeUrl` on `mountPreview`, `LumenPreviewSession`, and the React and Svelte `Player` names one directory serving `worker.js`, `lumen_wasm_bg.wasm`, `audio-worker.js` and `audio-worklet.js`, and every runtime URL not passed on its own comes from it, with its query kept on each file. With it, `wasmUrl` and `workerUrl` are optional. Without it nothing changes.
  - The wasm controller gains `setEnvironment`, `hasEnvironment` and `removeEnvironmentSource`, and `LumenMediaTarget` gains them as optional members. `setModel` and `setEnvironment` throw, with `media`, on a file they can't read.

  Breaking changes, each where the old behaviour differed between kinds:

  | Before                                                                                                                                | After                                                                                                                                                                                   | Migrating                                                                    |
  | ------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------- |
  | Every `media` entry was fetched, and one whose kind couldn't be told failed the load with "needs a kind".                             | Only the entries the composition reads load. When an expression computes a source, each entry is also read as the kind it reads, unless the entry's `kind` or blob type says otherwise. | Give entries of other kinds a `kind` when an expression computes a source.   |
  | A `kind` on an entry chose how it was read.                                                                                           | A `kind` the composition doesn't read the source as fails as `media.wrong_kind`.                                                                                                        | Drop the `kind`, or fix it to the kind the node reads.                       |
  | A source written as a URL (`https://…`), with another `scheme://`, empty or with a `..` part loaded if `media` had an entry under it. | It fails as `media.not_found`, as it does on `lumen-server`.                                                                                                                            | Name the source (`intro.mp4`) and pass the URL in `media` under that name.   |
  | Images, videos, models and fonts held the last frame while a composition loaded, but playback ran on and audio loaded on its own.     | Every kind holds the frame, and playback pauses while any source, audio included, is pending.                                                                                           | None needed. Await `whenReady()` before `play()` to start without the pause. |
  | Every `media` and `fonts` entry stayed registered while the page passed it, named by the composition or not.                          | Only what the composition names stays registered; the rest is unregistered and its memory freed. A source the old and new compositions share stays loaded.                              | None needed.                                                                 |
  | `LumenPreviewTransport` had `pause`, `play` and `seek`.                                                                               | It also has `whenReady(): Promise<LumenPreviewError \| null>`.                                                                                                                          | A custom transport passed to `preview.attach()` implements `whenReady`.      |
  | `wasmUrl` and `workerUrl` were required on `LumenPreviewSessionOptions`.                                                              | Both are optional when `runtimeUrl` is set; without either, the session fails to start with `worker_failed`.                                                                            | None needed.                                                                 |

- c0f96e6: `render_3d` can output its scene's depth or view-space normals as a raster for 2D effects such as fog, depth of field or relighting. Its new `output` param is `"color"` (the default, which renders as before), `"depth"` or `"normal"`. A node still has one output: a composition that wants colour and depth wires the same scene into two `render_3d` nodes, and the second shares the models, meshes and image-plane sources the first loaded.

  - `depth` is the distance along the camera's view, mapped from 0 at `depth_near` to 1 at `depth_far` (each 0 by default, which uses the camera's `near` or `far`), as opaque grey, and 1 where nothing is drawn. Rasters hold half floats, finest near 0, so set the range around the part of the scene the effect needs.
  - `normal` is the shaded normal (after normal maps, morph targets and skinning, reversed on the back of a double-sided surface) in the camera's space, +X right, +Y up, +Z towards the camera, stored as `n * 0.5 + 0.5`, opaque, and transparent where nothing is drawn.

  Both are stored as data, with no transfer function (an 8-bit output encodes them as it does light, so a depth of 0.5 is code 188), skip lighting, shadows and the environment, and ignore `supersample`: each pixel is the nearest surface at its centre. Opaque surfaces write them, masked ones where they reach their cutoff, and blended ones, such as image planes, where their alpha reaches half.

  Fog with existing nodes, `merge` with a luma mask: the scene is drawn twice, the colour render is the merge's `base`, a background is its `overlay`, and the depth render is its `mask`.

  ```json
  {
    "nodes": [
      { "id": 1, "type": "shape_3d" },
      { "id": 10, "type": "render_3d" },
      {
        "id": 11,
        "type": "render_3d",
        "params": { "output": "depth", "depth_near": 3, "depth_far": 14 }
      },
      { "id": 12, "type": "background", "params": { "paint": [205, 215, 228, 255] } },
      { "id": 13, "type": "merge", "params": { "mask_mode": "luma" } },
      { "id": 14, "type": "media_output" }
    ],
    "connections": [
      { "from_node": 1, "to_node": 10, "to_port": "scene" },
      { "from_node": 1, "to_node": 11, "to_port": "scene" },
      { "from_node": 10, "to_node": 13, "to_port": "base" },
      { "from_node": 12, "to_node": 13, "to_port": "overlay" },
      { "from_node": 11, "to_node": 13, "to_port": "mask" },
      { "from_node": 13, "to_node": 14, "to_port": "source" }
    ]
  }
  ```

- 5233133: 3D scenes, in a new `scene` node category. `shape_3d` makes a box, sphere, plane, cylinder, torus or cone; `camera_3d` is a perspective or orthographic viewpoint that can aim at a `target`; `light_3d` is a directional, point, spot or ambient light; and `merge_3d` groups any of them under one transform. Each outputs a scene, which only scene inputs take. `render_3d` draws the scene wired to its `scene` input into a raster with a transparent background, so it composites like any other layer: metallic-roughness lighting from the scene's lights and a built-in studio environment whose brightness is `environment`, opaque objects hiding what is behind them and blended ones (`opacity` below 1) drawn over them from back to front, with `exposure`, `tone_mapping` (`none`, `aces` or `agx`), fog, and `supersample` for antialiasing. A scene without lights gets a default key light and ambient fill, and one without a camera is seen from 5 units along +Z.

  World units are metres, Y is up and cameras look down -Z. `position`, `rotation` and `scale` are `[x, y, z]` values, so one expression can move all three axes, and a shape's `size` is its width, height and depth. `rotation` is in degrees, applied as yaw about Y, then pitch about the turned X, then roll about the turned Z. A light's `intensity` is a unitless multiplier.

  ```json
  {
    "nodes": [
      {
        "id": 1,
        "type": "shape_3d",
        "params": {
          "shape": "torus",
          "rotation": "=vec3(70, frame * 3, 0)",
          "color": "#e6b45a",
          "metallic": 1,
          "roughness": 0.25
        }
      },
      { "id": 2, "type": "camera_3d", "params": { "position": [0, 1.5, 4], "look_at": true } },
      { "id": 3, "type": "merge_3d" },
      { "id": 4, "type": "render_3d", "params": { "tone_mapping": "agx" } },
      { "id": 5, "type": "media_output" }
    ],
    "connections": [
      { "from_node": 1, "to_node": 3, "to_port": "objects" },
      { "from_node": 2, "to_node": 3, "to_port": "objects" },
      { "from_node": 3, "to_node": 4, "to_port": "scene" },
      { "from_node": 4, "to_node": 5, "to_port": "source" }
    ]
  }
  ```

  A `render_3d` whose scene has no expressions is kept across frames like any other still layer. Wiring a scene into a raster input fails `connection.wrong_kind`, whose message says to draw it through a `render_3d`. Rust embedders get the rendering behind the engine's new `3d` feature, which `lumen-server` and the WebAssembly bindings turn on; without it, a composition with any scene node fails `check` with the new `node.unsupported` code. The maths and meshes that need no GPU are in the new `lumen-engine-3d` crate.

- fe146b5: **Breaking:** `lumenSchemaVersion` is now the Lumen `major.minor` a composition was written for, `"0.6"` for the 0.6 line, instead of the frozen `"0.1.0"`. An engine reads only its own line: it accepts its `major.minor` or a full `major.minor.patch` of that line (`"0.6.3"`), and rejects everything else, including today's `"0.1.0"`, with `document.schema_version`:

  ```
  this composition declares schema version 0.1.0; this engine reads 0.6, so it was written for a different Lumen version
  ```

  Every stored composition that declares `"lumenSchemaVersion": "0.1.0"` must change it to `"0.6"`. A composition that leaves the field out is still read as the engine's own version. 0.x minor releases are breaking, so there is no migration between lines.

  `Composition.toJSON()` in the SDK always writes the current version, so compositions built with the SDK need no change, and the `lumenSchemaVersion` option of `Composition` is removed. `@lumiscia/lumen-types` exports `lumenSchemaVersion` (`"0.6"`), `lumen_engine::SCHEMA_VERSION` is the Rust side, and the JSON schema constrains the property to `^0\.6(\.[0-9]+)?$`. `just set-schema-version <major.minor>` moves the engine constant, the tracked fixtures and docs, and the generated output together.

- 79817d9: **Breaking (HTTP, `lumen-server`):** renders can be cancelled, the queue is bounded, and a render can end as `cancelled`.

  `POST /renders/:id/cancel` cancels a queued or running render. A queued render ends as `cancelled`, gives its queue place back and never starts (`200`); a running one answers `202` and stops at its next frame boundary (or at once while it waits for GPU device room, and during the audio and encoder finish after the last frame), closing its encoder, deleting its temporary output, releasing its media and freeing its slot. A render that has ended is returned unchanged with `200`, so the request is idempotent, and an unknown id is `404`. A cancelled render has `status: "cancelled"` and the code `render_cancelled` (not retryable) in `error`, `progress.errorCode` and `retryable`; `GET /renders/:id/artifact` answers `409` with that code, and its socket sends `render.cancelled` and closes.

  At most `--max-queued-renders` renders (`LUMEN_SERVER_MAX_QUEUED_RENDERS`, default 32) wait for a slot. Past that, `POST /renders` answers `503` with `render_queue_full` at once and keeps nothing; `0` refuses every render that can't start immediately. Before, any number queued.

  **Migrate:** clients that polled until `succeeded` or `failed` must also stop on `cancelled`. Clients that submit more than 32 renders ahead of the running ones must retry on `503` or raise `--max-queued-renders`. `Mp4Render::open` takes a `CancelToken` as a new last argument (`CancelToken::new()` for none), and `Mp4Render::set_cancel` is gone.

  TypeScript SDK: `RenderStatus` gains `"cancelled"`, and `lumen.cancelRender(id)` calls the route. `waitForRender` and `download` throw a `LumenError` with the non-retryable code `render_cancelled` for a cancelled render, where before a `cancelled` status was rejected as `invalid_response`.

  ```ts
  import { LumenError } from "@lumiscia/lumen-sdk";

  const render = await lumen.render(composition);
  await lumen.cancelRender(render.id);
  await lumen.waitForRender(render).catch((error) => {
    if (!(error instanceof LumenError && error.code === "render_cancelled")) throw error;
  });
  ```

  ```sh
  curl -X POST -H "Authorization: Bearer $TOKEN" http://127.0.0.1:8080/renders/r_123/cancel
  ```

  The `lumen_server::render` library gets the same hook: `RenderOptions::cancel` takes a `CancelToken`, and `Mp4Render::open` takes one for a render opened by hand. A render checks it while waiting for GPU device room, before each frame and while finishing, and ends with the new `ErrorCode::Cancelled` (`render_cancelled`). `RenderOptions` has a new field, so code that builds it without `..RenderOptions::default()` must add `cancel`.

  ```rust
  let cancel = CancelToken::new();
  let options = RenderOptions { cancel: cancel.clone(), ..RenderOptions::default() };
  // elsewhere: cancel.cancel();
  ```

  The CPU upload encode path reads frames back through a ring of three buffers while the next frame renders, instead of waiting for each readback. Output is byte-identical; memory is bounded by the ring.

- bc6b572: **Breaking (Rust, `lumen-server` library):** `lumen_server::render` reads no environment variables, and its error has a code to match on.

  A `RenderConfig` holds what renders take from the host: `font_dir`, `hardware_decode`, `gpu`, a `GpuSettings` (backends, power preference, fallback adapter), and `temp_dir`, where a render keeps its output while encoding (unset, the system temporary directory). Its default has no font directory, decodes video on the CPU and picks the high-performance adapter on any backend. The `lumen-server` and `lumen` binaries build it from `LUMEN_FONT_DIR`, `LUMEN_HARDWARE_DECODE` and `GpuSettings::from_env()` (`WGPU_BACKEND`, `WGPU_POWER_PREF`, `LUMEN_GPU_FORCE_FALLBACK_ADAPTER`, ...) as before, so they behave the same; `lumen-server` also takes `--font-dir`. `lumen-gpu` and `lumen-engine` gain `GpuSettings` and `new_with_settings` constructors that read nothing; `Renderer::new()` and `GpuCompositionRenderer::new()` still read the environment through `GpuSettings::from_env()`.

  `QualityConfig::resolve` in `lumen-ffmpeg` no longer reads `LUMEN_VIDEO_ENCODER`. A request pins the encoder it names, and a binary that wants the variable calls `QualityRequest::with_env_encoder()` first, which keeps the request's own encoder ahead of the variable and ignores an empty one. `lumen-server` and `lumen` do, so they behave the same; a render without `RenderOptions::encoder` uses the built-in presets. `LUMEN_MEDIA_ROOT` is gone: `RenderOptions::media_root` defaults to the current directory. `font_dir_from_env` is removed.

  Before:

  ```rust
  // LUMEN_FONT_DIR, LUMEN_HARDWARE_DECODE and LUMEN_MEDIA_ROOT were read from the process.
  let options = RenderOptions { media_root: Some(dir), ..RenderOptions::default() };
  let store = LocalMediaStore::new(root, font_dir.as_deref());
  let diagnostics = check_composition(&payload, &staged, &assets);
  match render_project_mp4(&composition, &options, &mut |_| {}) {
      Err(RenderError { code: "render_failed", message, retryable, .. }) => { /* any failure while rendering */ }
      ...
  }
  ```

  After:

  ```rust
  use lumen_server::render::{ErrorCode, RenderConfig, RenderOptions, render_project};

  let config = RenderConfig { font_dir: Some("/srv/fonts".into()), hardware_decode: true, ..RenderConfig::default() };
  let options = RenderOptions { config: config.clone(), media_root: Some(dir), ..RenderOptions::default() };
  let store = LocalMediaStore::new(root, &config);
  let diagnostics = check_composition(&payload, &staged, &assets, &config);
  match render_project(&composition, &options, &mut |_| {}) {
      Err(error) if error.code() == ErrorCode::Engine("media.not_found") => { /* error.engine_error() */ }
      Err(error) if error.code() == ErrorCode::EncodeFailed && error.retryable() => { /* retry */ }
      Err(error) => eprintln!("{}: {error}", error.code().as_str()),
      Ok(rendered) => { /* rendered.bytes */ }
  }
  ```

  `RenderError` is renamed `ServerRenderError`, so it no longer shares a name with `lumen_engine::RenderError`. Its fields are private: `code()` returns an `ErrorCode`, `retryable()` says whether to try again, `Display` is the message, and `source()` is the cause. An engine failure keeps the engine error as its source and has its diagnostic code, such as `media.not_found`, `media.decode` or `render.gpu`; of those `render.gpu` is retryable, and a media failure only when it is a shortage that can pass, such as running out of file descriptors. The library's own codes are `invalid_project_payload`, `invalid_quality`, `invalid_render_profile`, `invalid_config`, `render_failed`, `encode_failed`, `output_io`, `invalid_output`, `output_too_large` and `render_cancelled`. `audio_render_failed` is now the engine's code when mixing the audio failed and `encode_failed` when encoding it did, `invalid_fps` is `invalid_render_profile`, and `media_root_error` and `font_dir_error` are `invalid_config`. An encoder that fails to start keeps the FFmpeg error as its source: failing on the output is `output_io`, running out of memory, file descriptors or hardware encoder sessions is retryable, and an invalid option or unsupported encoder is not.

  The HTTP server's failed renders carry these codes in `render.error.code`, `progress.errorCode` and the `render.failed` event, so a render failing on a missing or corrupt source reports `media.not_found` or `media.decode`, not `render_failed`, and is no longer marked retryable.

- 0382574: 3D lights can cast shadows. Turn on `cast_shadows` on a directional, spot or point `light_3d`, and objects between it and others block its light. `shadow_softness`, 0.02 by default, is how wide in world units shadow edges blur. A directional light's shadows cover what the camera sees of the scene out to `shadow_distance`, 50 by default, measured from the camera along its view, and fade out over its last tenth. Its shadow map spreads over that much of what the camera sees, so a larger distance trades sharpness for reach, while a scene the camera sees nearer keeps its full sharpness. A spot light's shadows cover its cone, up to 170° across, and a point light's reach every direction. A `render_3d` draws shadows for the first 8 lights casting them in its scene, and the rest light the scene without shadows, named in a `scene.shadow_limit` warning. A scene with no light casting shadows renders exactly as before.

  `shape_3d`, `image_plane_3d` and `model_3d` take `cast_shadows` and `receive_shadows`, both on by default. Unlit and glowing surfaces cast shadows too, so a shape standing in for a light, such as a bulb around a point light, needs `cast_shadows` off. Masked surfaces cast shadows only where they aren't cut away, blended ones where their alpha is at least 0.5, a model's in the shape of its current pose, and an image plane's in the shape of its raster's alpha.

  On `shape_3d` and `image_plane_3d`, `shadow_catcher` shows nothing but the shadows falling on the surface: transparent black that darkens what the render is composited over by as much light as the shadows take away, times the colour's alpha, `opacity` and an image plane's raster's alpha, whether or not it is `unlit`. It shows none of its colours, hides what is behind it wherever that alpha isn't 0, and casts no shadows. That grounds a model on 2D footage or a flat background:

  ```json
  {
    "nodes": [
      { "id": 1, "type": "background", "params": { "paint": [70, 130, 160, 255] } },
      {
        "id": 2,
        "type": "model_3d",
        "params": { "source": "asset:fox", "animation": "Walk", "scale": [0.02, 0.02, 0.02] }
      },
      {
        "id": 3,
        "type": "shape_3d",
        "params": { "shape": "plane", "size": [10, 1, 10], "shadow_catcher": true }
      },
      {
        "id": 4,
        "type": "light_3d",
        "params": {
          "light": "directional",
          "intensity": 3,
          "rotation": [-55, -40, 0],
          "cast_shadows": true,
          "shadow_softness": 0.04
        }
      },
      { "id": 5, "type": "light_3d", "params": { "light": "ambient", "intensity": 0.4 } },
      {
        "id": 6,
        "type": "camera_3d",
        "params": { "position": [0, 1.4, 4.2], "look_at": true, "target": [0, 0.7, 0] }
      },
      { "id": 7, "type": "merge_3d" },
      { "id": 8, "type": "render_3d" },
      { "id": 9, "type": "merge" },
      { "id": 10, "type": "media_output" }
    ],
    "connections": [
      { "from_node": 2, "to_node": 7, "to_port": "objects" },
      { "from_node": 3, "to_node": 7, "to_port": "objects" },
      { "from_node": 4, "to_node": 7, "to_port": "objects" },
      { "from_node": 5, "to_node": 7, "to_port": "objects" },
      { "from_node": 6, "to_node": 7, "to_port": "objects" },
      { "from_node": 7, "to_node": 8, "to_port": "scene" },
      { "from_node": 1, "to_node": 9, "to_port": "base" },
      { "from_node": 8, "to_node": 9, "to_port": "overlay" },
      { "from_node": 9, "to_node": 10, "to_port": "source" }
    ]
  }
  ```

  The GPU recorder can draw into a depth target alone and sample it with a comparison sampler.

- f045a94: **Opt-in 10-bit video output.** A render can ask for 10 bits per sample with `hevc` or `av1`: the request's `bit_depth`, `lumen --bit-depth 10` or the SDK's `render(..., { bitDepth: 10 })`, which win over `render_settings.bit_depth`, which wins over a custom preset's `bit_depth` in the quality config, as `codec` does. The default stays 8 everywhere.

  ```json
  { "render_settings": { "width": 1920, "height": 1080, "codec": "hevc", "bit_depth": 10 } }
  ```

  The renderer quantises its float working space straight to 10-bit BT.709 limited range (Y 64 to 940, C 64 to 960), with the same ordered dither as 8-bit. `libx265` and `libsvtav1` encode `yuv420p10le` (HEVC Main 10, AV1 Main); `hevc_vulkan`, `av1_vulkan` and NVENC take P010 where the device supports it. H.264 is always 8-bit and VideoToolbox has no 10-bit path, so asking either for 10 bits is an error. An encoder that can't write 10 bits is skipped like one that won't open, and a pinned one fails the render: a 10-bit request never comes out at 8.

  **CPU encoders now get dithered YUV from the GPU.** Every encoder that takes CPU frames, at either depth, receives YUV 4:2:0 converted on the GPU from float colour and quantised once, so 8-bit CPU output is dithered in its final domain too and no longer passes through swscale. Its chroma is the 2x2 average of encoded colour, as the Vulkan and NVENC paths already were, so 8-bit `libx264`, `libx265` and `libsvtav1` output changes slightly.

  **Rust API:** `VulkanVideoTarget::nv12(renderer, size)` is `VulkanVideoTarget::new(renderer, size, BitDepth, dither)`, and `CudaNvencTargetPool::rgba8(renderer, size)` is `CudaNvencTargetPool::new(renderer, size, BitDepth, dither)`. `VulkanVideoFrame::new` takes a `VulkanFrameLayout` (`nv12` or `p010`) in place of a width and height. `lumen_engine::gpu::YuvConverter` converts a frame to `Nv12`, `P010`, `Yuv420p` or `Yuv420p10`. In `lumen-ffmpeg`, `VideoEncoderConfig::bit_depth`, `VideoEncoderChoice::with_bit_depth`, `QualityRequest::bit_depth`, `check_bit_depth` and `MuxedEncoder::cpu_input_format` are new, `PixelFormat` has `Yuv420p` and `Yuv420p10`, and `write_video_frame` takes YUV frames in the encoder's own format as well as RGBA8, which a 10-bit encoder refuses.

  Output streams are now tagged with centred chroma (`chroma_location=center`), which is where the 2x2 average puts it. An encoder option that would change the pixel format, and with it the bit depth, is an error when the encoder opens.

- ef94bcb: `text_3d` extrudes text into a solid in a 3D scene. It shapes `content` as a `text` node with the same font and layout would, with its kerning, ligatures, font fallback and wrapping, then extrudes each glyph's outline `depth` deep along Z, centred on `position`, with front and back caps that keep counters open. `bevel_size` and `bevel_segments` round the edges off in a quarter-round profile, or a flat 45° chamfer at 1 segment; a glyph whose strokes are too thin for the bevel gets the widest one that fits it. It takes the `position`, `rotation`, `scale` and material params of a `shape_3d`.

  `font_size` is in world units, the height of the em square, so at the default of 1 a word stands about as tall as a default `shape_3d` box is wide. `max_width`, `letter_spacing` and `line_height` are in world units too. `alignment_horizontal` and `alignment_vertical` align lines as in 2D and choose which point of the text's box sits at `position`, its centre by default.

  Geometry is built in ems and kept per glyph, so repeated glyphs share one mesh and animating `font_size`, or `depth` of text without a bevel, builds nothing again; animating a bevelled text's `depth`, `bevel_size` or `font_size` builds its glyphs again. Glyph meshes are kept up to 64 MiB, least recently used first. Animating a `text_3d`'s transform or material costs no more than a `shape_3d`'s.

  Text is bounded while it is built. A node draws at most as many glyphs as the host's text glyph limit allows a `text` node, and at most 4000000 vertices and 12000000 indices, which the build counts glyph by glyph and stops at the first to pass; more is a render error. One glyph's outline may have 4096 commands, 64 contours and 8192 flattened points, and its mesh 262144 vertices and 786432 indices, and 3072 points where it is bevelled; a font with a glyph past one of these fails the render naming the limit, rather than the glyph being left out. A node's distinct glyphs together are bounded too, by the squares of their flattened points adding up to at most four glyphs at the point limit; every glyph of Roboto's Latin and symbol blocks takes under 1% of that. Real glyphs are far below them: the most complex Latin glyph of Roboto or Inter has 82 commands, 10 contours and 199 points, and 13924 vertices at 16 bevel segments.

  `font_size`, `max_width`, `line_height`, `depth` and `bevel_size` may not be negative, and none of these or `letter_spacing` may exceed 1000000 world units or, apart from `font_size`, 100000 times `font_size`; `font_weight` is 1 to 1000. `check` reports a literal outside them as `node.invalid_param`, and an expression that evaluates outside them is a render error naming the param. A `font_size` under 0.000001 draws nothing. Glyphs drawn from colour or bitmap tables, such as emoji, have no outline, so they are left out with a new `font.no_outlines` warning; a font that mixes colour glyphs with ordinary letters still outlines the letters. `check` reports a family no font answers to as `font.fallback`, as for `text`. Rich text spans, decorations, strokes and the `glyph_*` params of `text` have no counterpart.

  ```json
  {
    "nodes": [
      {
        "id": 1,
        "type": "text_3d",
        "params": {
          "content": "Lumen",
          "depth": 0.3,
          "bevel_size": 0.02,
          "rotation": [-20, 0, 0],
          "color": [232, 176, 96, 255]
        }
      },
      { "id": 2, "type": "light_3d" },
      { "id": 3, "type": "merge_3d" },
      { "id": 4, "type": "render_3d" },
      { "id": 5, "type": "media_output" }
    ],
    "connections": [
      { "from_node": 1, "to_node": 3, "to_port": "objects" },
      { "from_node": 2, "to_node": 3, "to_port": "objects" },
      { "from_node": 3, "to_node": 4, "to_port": "scene" },
      { "from_node": 4, "to_node": 5, "to_port": "source" }
    ]
  }
  ```

- 91a0446: A text node draws up to 16384 glyphs, strokes and decorations by default instead of 2048, so a full 1920 × 1080 frame of 16 px monospace text (200 × 68 glyphs) is one node. The limit is a host setting: `--max-text-glyphs` (`LUMEN_MAX_TEXT_GLYPHS`) of `lumen`, `--max-text-glyphs` (`LUMEN_SERVER_MAX_TEXT_GLYPHS`) of `lumen-server`, and `GpuCompositionRenderer::set_max_text_glyphs` in `lumen-engine`, up to 1048576. A text over the limit is still a render error before its glyphs are drawn, and the error now names the limit and how a host raises it. Compositions that split text into nodes of under 2048 glyphs render as before.
- 49b2878: Node inputs and outputs have a kind, `raster` or `scene`, so a scene can't be wired where a raster goes or the reverse. A connection joining a node's output to an input of another kind fails validation with `connection.wrong_kind`, naming both nodes, the input and the two kinds, and its diagnostic points at the connection. The generated definitions give each port's and output's kind as `raster` or `scene`, where they said `raster_frame`, and list the kinds as `x-lumen-portKinds`; `NodePortKind` is `"raster" | "scene"`, and `NodeOutputKind` and `NodeInputPortKind` give a node kind's output and an input's kind. In the SDK, `addNode` inputs and `connect` only take nodes whose `type` outputs what the port takes. `NodeReference<"scene">` or `NodeReference<"raster">` is a node for a port of that kind, and a plain `NodeReference` is any node; a bare id isn't checked.
- 3ab8aa0: Params can now hold three numbers, written `[x, y, z]` in JSON or as an expression, ready for the 3D nodes' `position`, `rotation` and `scale`. Expressions gain `vec3(x, y, z)`, and vec3 values work like vec2 values: they add and subtract with each other, multiply and divide componentwise or by a number on either side, negate, and interpolate in `lerp`, `tween` and `keyframes`, e.g. `"=tween(vec3(0, 0, -400), vec3(0, 0, 0), 0, 24, 'out_expo') + vec3(0, 10 * sin(time), 0)"`. Mixing a vec3 with a vec2 is an error. Template props can be declared `"type": "vec3"`, and a `wgsl_shader` vec3 uniform takes a vec3 expression. In `@lumiscia/lumen-shared` the helper is `vec3`, which returns an `Expr<Vec3Value>` that TypeScript keeps apart from colour expressions, so a vec3 can't reach a colour param or share a `lerp` or keyframe track with a colour. `prop.vec3(..)` declares a prop, and params and prop defaults take a plain `Vec3` tuple `[x, y, z]`. A three-number tuple passed to a helper still means `rgb(..)`, so write `vec3(x, y, z)` there. `vec3` is now a reserved name, so composition functions and their parameters can't use it.
- b9022b7: Video follows its primaries and transfer tags, and HDR video is tone-mapped to SDR. Until now every video was read as SDR BT.709 whatever it was tagged, so PQ and HLG footage, such as iPhone HDR clips, looked dark and flat, and BT.2020 or P3 footage looked dull. No params change: a tagged source just renders right.

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

  These conversions apply to CPU decode and both hardware decode paths, Vulkan Video and VideoToolbox, which share one shader.

  **Rust API:** `lumen_ffmpeg::ColorTags` gains `primaries`, `transfer` and `chroma_location`, with `ColorPrimaries`, `ColorTransfer` and `ChromaLocation`, and `ColorTags::new(matrix, full_range)`; `VideoStreamInfo` has the stream's `color`. `lumen_engine::media::YuvColor` gains `primaries: ColorPrimaries` and `transfer: TransferFunction`, and `YuvColor::sdr(matrix, full_range)` builds the SDR BT.709 case. `VideoFrameResolver::color_warning` (default `None`) reports tags a resolver reads without. Code that builds these structs by their fields needs the new fields.

  RGB and grey video (FFV1 `gbrp`, for example) decodes to 8-bit RGBA read as sRGB, so its primaries and transfer are not converted; an HDR or wide-gamut stream of that kind gets the same `media.color_unsupported` warning. The warning is decided from the stream's tags when it opens: a frame whose own tags differ from the stream's is converted by its own tags but does not change the warning.

### Patch Changes

- d09b553: Documentation for 3D. The `lumen-sdk`, `lumen-shared` and `lumen-preview` READMEs link to the new 3D guide, and `lumen-shared`'s shows a scene. The descriptions of a 3D material's `color` and `shadow_catcher` params, which `lumen-types` carries as doc comments and `lumen-bindings` as node metadata, now say that only an image plane has a raster to multiply by `color` or to count in `shadow_catcher`. Nothing about how a composition renders changes.
- ad144c3: The engine's frame graph dispatches to each node through one `RecordNode` implementation that the node list generates, and keeps what recording holds across frames in separate parts for retained outputs, text, media, runtime shaders and 3D. Nothing renders differently, and performance is unchanged.
- 267dca6: The GPU recorder can draw with a depth attachment and back-face culling, bind long-lived storage buffers and unfilterable float textures, and encodes consecutive draws onto the same attachments in one render pass. Nothing renders differently; this is groundwork for 3D.
- Updated dependencies [d09b553]
- Updated dependencies [8ef4ee5]
- Updated dependencies [97c7016]
- Updated dependencies [89568da]
- Updated dependencies [6528c7b]
- Updated dependencies [a37b057]
- Updated dependencies [6487216]
- Updated dependencies [1879afe]
- Updated dependencies [d247928]
- Updated dependencies [d63ba1d]
- Updated dependencies [7a87028]
- Updated dependencies [1badd98]
  - @lumiscia/lumen-preview@0.6.0-beta.0

## 0.5.1

### Patch Changes

- bbab23f: Renders request the GPU's real device limits instead of wgpu's conservative defaults, so canvases and images larger than 8192 pixels work on GPUs that support them.
- d14deff: Color grade, crop and the composition output read a small layer at its own size instead of first copying it into a full-canvas texture, which saves a full-frame pass per use.
- 14af846: Videos cut with `ffmpeg -c copy` no longer count their hidden preroll frames, so renders near the end of the clip work.
- 6f5b681: The wasm build no longer includes wgpu's WebGL backend, making the preview's wasm smaller. The preview already required WebGPU, so behavior is unchanged.
- 6285ab0: Expressions gain the `sine`, `quart`, `quint`, `circ`, `elastic` and `bounce` easing families (`in_`, `out_` and `in_out_` each, e.g. `out_bounce(t)` or `tween(0, 1, 0, 30, 'in_out_elastic')`), and the functions `sign`, `atan2`, `hypot` and `remap`. `@lumiscia/lumen-shared` exports a helper for each.
- 6a6ee44: Load fonts faster: each font file is parsed once instead of on every later font load.
- 10b9793: Fix clippy lints in the FFmpeg video resolver that only show up when `lumen-server` is built without the `vulkan` feature.
- 87164d8: Render errors name the node by its JSON `type`, like `media_in node 3 evaluation failed`, instead of a Rust-style `MediaIn`. `motion_blur` now says in its description that it blurs content that already has motion blur a second time, and `lumen-local --help` no longer reads as if a quality preset targeted 14 Mb/s.
- 5406425: Rendering a canvas larger than the device's texture limit, including a clip's template that declares one, now returns a render error naming the node and the limit instead of panicking.
- 7fcff92: A composition nested more than 48 nodes deep now fails with a `render.too_deeply_nested` error that names the node, instead of crashing the process with a stack overflow.
- ee8b9bf: A renderer reused for another composition no longer keeps the previous one's path and font caches, and video frames that already match the canvas size are uploaded without an extra copy.
- f9952fa: Keep static subtrees that read an image between frames instead of drawing them again every frame, and draw them again when the image under their source is replaced.
- 9cd9f55: `lumen-server` no longer parses and checks a composition on the async runtime when a render is created, and concurrent renders of the same media now share one resolver instead of each keeping its own.
- @lumiscia/lumen-preview@0.5.1

## 0.5.0

### Minor Changes

- 6cddffa: A `motion_blur` node blurs any motion in its source, including what the analytic `motion_blur` shutter of shapes, paths, text and `transform` can't see: animated `width`, `height`, `font_size` and `border_radius`, path data, anything upstream of a `transform`, and motion inside video. `{ "type": "motion_blur", "params": { "samples": 8, "shutter": 180 } }` renders its source at `samples` sub-frame times across a `shutter` of that many degrees, centred on the frame like the analytic shutter, and averages them in premultiplied space. The result is the average of the sub-frames' 8-bit renders, rounded to the nearest level. Each sample renders the whole source again, so the cost grows with `samples`; the samples share the same pooled textures, so memory doesn't. Video shows the source frame each sub-frame time maps to, and media prediction asks for those frames. `samples` is 1 to 64 and `shutter` 0 to 360; other values fail the render. `samples: 1` or `shutter: 0` renders the source once, unchanged. While a `motion_blur` samples, analytic motion blur inside it is off, and a `motion_blur` inside it renders its source once per sample.
- db420ff: Source fields accept asset references, `asset:<id>`, naming a file in the host's asset store: `media_in`'s `source`, an audio clip's `source_id`, and a text node's or span's `font_family`. The `asset:` prefix is a scheme, so it can't be mistaken for a relative path, and plain paths and the `media` manifest keep working as before. An id the host doesn't have is an `asset.not_found` error, and an asset of another kind than its field reads, such as a font used as an image, is `asset.wrong_kind`. A host gives the engine its store as an `AssetResolver`, which resolves an id to an `Asset` (an image, video or audio file, or a font family's face files), and passing it to `json::check` checks literal references before rendering. `lumen-local` has no asset store, so references are `asset.not_found` errors there, and `lumen-server` resolves them against its media uploads.

  A font asset is one family, and weight and style choose among its faces. Fonts now shape under the name the composition asks for: a font staged under a `media` alias, or any file whose faces carry another family name, used to load and then silently fall back, and now draws as asked. Its faces answer to that name instead of their own, so they can't stand in for another font with the same internal family name. For Rust users, `MediaStore::get_font_resolver` returns a `Result`, and `json::check` and `check_value` take the asset store as a third argument.

- d21228a: `path` strokes take a dash pattern. `stroke_dash` lists up to 16 non-negative lengths in untransformed pixels that the stroke alternately draws and skips, starting with a dash; an odd number of lengths repeats, as in SVG. Each length takes a literal or an expression. Every dash has round caps, so a dash of length 0 draws a dot, and dashes that cross a corner join as a solid stroke does. `stroke_dash_offset` shifts the pattern along each contour and takes an expression, so `"=frame"` marches the dashes. Dashes are measured along the whole contour, and `trim_start` and `trim_end` then clip them. An empty or all-zero pattern strokes solidly. Undashed strokes no longer distort segments shorter than 0.01 units, and an untrimmed stroke now draws the whole of a segment that is very short next to its contour, where it was drawn only halfway.
- 4df3509: Diagnostics reach the TypeScript packages. The wasm bindings add `check(json, fonts?)`, which returns what is wrong with a composition without loading it, and a composition `loadComposition` rejects throws with the same `diagnostics`. `LumenError` in the SDK has `diagnostics`, filled when a render is rejected for an invalid composition. The preview and both `Player` components take `onError`, which receives each failure with its diagnostics. `LumenDiagnostic` is exported from `@lumiscia/lumen-types`.
- 428e0a7: `directional_blur`, `radial_blur` and `zoom_blur` nodes average their source along a path through each pixel, centred on it. `directional_blur` spreads each pixel along a line `distance` pixels long at `angle` degrees clockwise from horizontal. `radial_blur` spins it along an arc of `amount` degrees, up to 360, about `center`, and `zoom_blur` streaks it along the ray from `center` over `amount` times its distance from it. `edge` is `clamp`, which repeats the canvas's edge pixels, or `transparent`. `zoom_blur` samples its streak in equal ratios of scale, so it reaches farther away from `center` than towards it. Each blur takes about one sample per pixel of its longest streak, up to 4096, so streaks up to 4096 pixels long stay smooth; streaks longer than about 256 pixels are taken in two passes whose second resamples the first, which softens sharp edges by a few levels more than single samples would, and streaks from 64 to 256 pixels crossfade between the two so that animating a blur never jumps. The output is sized to the source grown along the path it is blurred over.
- 8a28aa1: A `displacement_map` node moves each pixel of its `source` input by the pixel of its `map` input over it, in a single pass. `channel_x` and `channel_y` pick the map channel that drives each axis, `red`, `green`, `blue` or `luma`, and `amount_x` and `amount_y` are the most pixels it moves: a byte of 0 moves the source the whole amount left or up, 255 nearly as far right or down, and 128 leaves it in place. Map bytes are premultiplied by alpha, so transparent areas of the map, and everything outside it, displace nothing, and a neutral grey stays neutral as it fades. `edge` is `clamp`, which repeats the source's edge pixels, or `transparent`, and `sampling` is `linear` or `nearest`. The output is sized to the source grown by the amounts where the map overlaps it.
- a08ab26: Expressions gain seeded `noise(seed, x)` and `random(seed, x)` builtins. `noise` is smooth 1D gradient noise in `-1..=1`; `random` is a uniform number in `0..1`, and `random(x)` uses seed 0. Both are built from integer hashing and exact float arithmetic, so a frame renders identically on every host.
- 08a87a7: Expressions gain `path_point(path_data, t)` and `path_angle(path_data, t)` for moving along a curve, e.g. `"position": "=path_point('M 0 0 C 100 0 100 100 200 100', t)"` with `"rotation": "=path_angle(..)"`. `t` is the fraction of the path's arc length, clamped to `0..=1`, so a steady `t` moves at a steady speed. `path_point` returns a vec2 on the curve; `path_angle` is the direction of travel in degrees clockwise from +x, like `rotation`, and counts whole turns instead of wrapping, so a clockwise circle ends at 360. Moves between contours add no length, and a closed contour's closing segment does. Path data is parsed like a `path` node's `data`, and each path is measured once and cached. In `@lumiscia/lumen-shared` they are `pathPoint` and `pathAngle`.
- 3861bfd: `lerp`, `tween` and `keyframes` interpolate path data, so a `path` node's `data` can morph: `"=lerp('M 0 0 L 100 0 L 50 80 Z', 'M 0 40 L 100 40 L 50 0 Z', ease('in_out_cubic', t))"`, or `"=keyframes(frame, key(0, 'M 0 0 L 100 0 L 50 80 Z'), key(30, 'M 0 40 L 100 40 L 50 0 Z'))"`. The two paths must have the same commands in the same order, with relative and absolute commands told apart and an arc's flags matching, and they interpolate point by point; otherwise the error names the first command that differs. Paths are not resampled to match. The result writes each command out, with numbers at the precision the `path` node draws with, and numbers that both paths share aren't mixed, so they don't jitter. Keys that don't interpolate, a `hold` or two keys at the same time, may switch between paths of any structure. Strings that aren't path data don't interpolate, and the error says which one is malformed. Literal paths are checked when the expression is parsed. In `@lumiscia/lumen-shared`, `lerp`, `tween` and `key` accept strings.
- 53e1e41: A `fractal_noise` source node generates opaque grey fractal noise on the GPU: 2D gradient noise, the per-pixel counterpart of the `noise()` expression builtin, summed over 1 to 8 `octaves` of halving size and strength. `scale` is the size of the largest features in pixels, `seed` picks an unrelated pattern, and `evolution` morphs it smoothly in turns, repeating every whole turn, so `"=time"` loops once a second. `mode` is `smooth` or `turbulent`, which folds each layer for dark creases. `contrast`, `brightness`, `offset_x` and `offset_y` adjust and move the pattern, and `width` and `height` size the output as on `background`. The noise is built from integer hashing, with its gradients and long offsets worked out on the CPU, so a pattern holds steady at any offset or time and GPUs agree to within a level of rounding.
- 3665a2c: `glow` takes an `edge` param like the blurs: `clamp`, the default, repeats the canvas's edge pixels as glow always has, and `transparent` lets the glow fade out towards the canvas edges. `radial_blur` and `zoom_blur` spin and zoom about the middle of the canvas, `[width / 2, height / 2]`, when `center` is left out, on any canvas size; before, they turned about pixel `[960, 540]`, which is off-centre on any canvas but 1920×1080.
- b7342d2: A `glow` node adds a blurred copy of its source's brightest pixels to it, as light. Luminance above `threshold` is remapped to full strength at white, so pixels at or below it, and black at any threshold, cast nothing, and a `threshold` of 1 lets nothing glow; past 254/255 white dims too, so the glow fades out continuously as the threshold reaches 1. The light is blurred with the `blur` node's Gaussian of `radius`, scaled by `intensity` (up to 1000), and added to the premultiplied source, clamped at full. With `tint_enabled`, the glow takes the solid `tint` colour at the luminance of the light that casts it. `glow_only` outputs the glow alone, whose alpha is its brightest channel. The output is sized to the source grown by the blur's reach.
- f31d22c: Shapes and paths take a `repeat` param that draws up to 1024 copies of the node in one pass. Copy `i` adds `i` times `rotation_offset`, `scale_offset` and `opacity_offset` to the node's own rotation, scale and opacity, about its own anchor, and moves by `position_offset` per column. With `columns` set, copies wrap into rows that move by `row_offset`. Each offset takes expressions, evaluated once per frame. Each copy's opacity is clamped to 0–1, later copies draw over earlier ones, and the node's texture covers only the copies that draw anything.
- 8857666: Expressions gain keyframe tracks: `keyframes(t, keys...)` or `keyframes(t, ease, keys...)`, with one `key(at, value, ease?)` per key. Values are numbers, vec2 values or colours, and colours mix premultiplied like `lerp`. A key's `ease` is how the curve leaves it toward the next key, and keys without one use the track's default, which is `'linear'`. Before the first key and after the last the value holds, and two keys at the same `at` jump.

  Easings are now values. An easing is a name such as `'out_expo'`, the new `'hold'`, which keeps the value until the next key and then jumps, or `cubic_bezier(out, in)`, which is CSS `cubic-bezier(out.x, out.y, in.x, in.y)` with `vec2` handles. `tween` takes any easing, and `ease(easing, t)` applies one to progress `t`. In `@lumiscia/lumen-shared`, `ease` is callable as `ease(easing, t)` and still holds the easing names, such as `ease.outExpo`, and `key` and `keyframes` helpers are added.

  **Breaking:** `cubic_bezier(x1, y1, x2, y2, t)` no longer parses. Write `ease(cubic_bezier(vec2(x1, y1), vec2(x2, y2)), t)` instead; the parse error says so. The TS helper changes to `cubicBezier(out, in)` with it. `ease`, `key` and `keyframes` are now reserved names, so composition functions and their parameters can't use them.

- 48ad23c: `media_in` takes an `alpha` param for video: `"straight"` (the default, as before) or `"premultiplied"`, for sources like many ProRes exports whose colour is already multiplied by alpha and would otherwise be premultiplied twice and show dark edges. For Rust users, `MediaFrame` has a new `RawRgba` variant for RGBA frames whose premultiplication is left to the node reading them; FFmpeg RGB video with alpha now comes out as one.
- c491e28: Merge and RasterMultiMerge gain the hue, saturation, color and luminosity blend modes.
- c667040: NVENC on the CUDA zero-copy path now encodes BT.709 like every other encoder: frames are converted to BT.709 NV12 on the GPU before NVENC and tagged `bt709`, instead of being converted by NVENC with BT.601 and tagged `bt470bg`.
- 1d76f93: `path` fills take an `offset` and a `feather`, both in untransformed pixels and both animatable. A positive `offset` grows the fill outward, rounding the corners it grows around, and a negative one shrinks it; a fill shrunk past its middle vanishes, and the stroke stays on the original outline. `feather` fades the fill's edge over that width, centred on the edge, without a blur pass. Under non-uniform scale both stretch with the path, as `stroke_width` does. At 0, the default, fills render as before, and an offset or feather animated to 0 meets them there. Edges that coincide and cancel under the even-odd rule, such as the side two adjoining squares share, are no longer edges of the fill: they are neither offset nor feathered, and sharp fills no longer show a faint seam along them.
- 29f9e9b: `Player` no longer takes `bindings`: `<Player preview={preview} composition={composition} />` is enough. React and Svelte now depend on `@lumiscia/lumen-bindings` and start its preview worker and wasm from where they're installed, which Vite and plain ESM both resolve. A page that serves them elsewhere passes `workerUrl` and `wasmUrl` to `Player`.

  This is breaking. `mountPreview` and `LumenPreviewSession` take `wasmUrl` and `workerUrl` in place of `bindings`, and `@lumiscia/lumen-bindings/urls` exports the defaults without loading the wasm. `LumenWorkerBindings` is gone, and `createLumenBindings` from `@lumiscia/lumen-bindings` no longer takes `previewWorkerUrl`; it only loads the wasm on the calling thread.

- 404f117: Renders pick their quality by preset and their codec by name. Every host defines `draft`, `standard`, `high` and `lossless`, each with settings per FFmpeg encoder, and `render_settings` in a composition gains `codec` (`h264`, `hevc` or `av1`) and `quality` (a preset name), which the `RenderSettings` type and `new Composition({ renderSettings: { codec, quality } })` carry. Templates' render settings are now `TemplateRenderSettings`, without them. Previews ignore both.

  A render with no preset now uses `standard` instead of a 14 Mb/s target. Each codec tries its encoders in order, with one warning for each that won't open: on Linux the Vulkan encoder, then `libx264`, `libx265` or `libsvtav1`, and on macOS VideoToolbox, then the same software encoders. A preset only tries the encoders it has settings for, so `lossless` uses `libx264`, `libx265` or NVENC H.264 and HEVC. `LUMEN_VIDEO_ENCODER` still pins one encoder with no fallback. On `lumen-server`, a preset, codec or pinned encoder this FFmpeg build can't serve is a `422` when the render is requested. An output container that can't hold the codec, such as H.264 in `.webm`, fails when the encoder is created.

  `LUMEN_QUALITY_CONFIG` (and `lumen-server --quality-config`) names a JSON file that sets the default preset and codec, each codec's encoder order, and presets' settings per encoder: `crf`, `qp` or `bitrate`, an encoder `preset`, and raw FFmpeg `options`. It is checked when `lumen-server` starts or `lumen-local` loads it, so an unknown field, codec, encoder or option, or a value outside an option's range, fails then instead of on a render.

  `lumen-local` gains `--quality`, `--codec` and `--quality-config`. `--bitrate`, `--crf`, `--qp` and `--preset` keep their meaning, replacing the preset on every encoder tried, and can't be combined with `--quality`. `--crf`, `--qp` and `--preset` now also work on `libx265` and `libsvtav1`.

  For Rust users of `lumen-engine-ffmpeg`, `QualityConfig` loads and resolves presets into a `VideoEncoderChoice`, which holds the ordered `EncoderCandidate`s a render tries; `EncoderQuality` gains raw `options`, and its `rate_control` is optional, so `EncoderQuality::default()` and `VideoEncoderConfig::cpu_rgba` leave rate control at the encoder's default. `VideoEncoderChoice::resolve` and `check_quality` are gone. In `lumen-server`, `RenderOptions::encoder` and `resolve_encoder` replace `video_encoder` and `quality`, and `Mp4Render::open` takes the choice.

- ef422d1: Radial gradients take an optional `focal` point, which makes them two-point conical gradients for off-centre highlights. The first stop sits at the focal point and the last on the circle at `center` and `radius`. A focal point on or outside that circle is moved just inside it. Without `focal`, or with `focal` equal to `center`, gradients render as before.
- 5db9958: Merge and RasterMultiMerge gain the add, difference, exclusion, color dodge, color burn, hard light and soft light blend modes.
- d60eaad: `POST /renders` on `lumen-server` accepts `quality`, a quality preset name such as `"high"`, and `codec`, `"h264"`, `"hevc"` or `"av1"`. Both win over the composition's `render_settings`. `RenderOptions.quality` and `RenderOptions.codec` in the SDK send them, and `QualityPreset` types the preset name.
- 991ef36: `lumen-server` keeps uploaded media and resolves `asset:<id>` references against it. `POST /media` with a single `PUT` or a multipart upload, and `POST /media/url`, take the same requests and answer the same shapes as the hosted API, so the SDK's `uploadMedia` and `createUrlMedia` work against it unchanged. Each file is checked once its bytes are complete, by opening it with the decoders renders use, and the media reports its `kind` (`image`, `video`, `audio` or `font`) and `metadata`, or `rejected` with a `rejectionReason`. `POST /renders` refuses a composition naming media that is missing, expired, pending, rejected or of the wrong kind with `422` and `asset.not_found` or `asset.wrong_kind` diagnostics, and `POST /renders/check` reports the same. Media is kept in `--media-dir` for `--media-ttl` (24 hours by default), up to `--media-max-bytes` per file, and a render keeps the assets it uses until it ends. At most `--media-max-pending` media (100 by default) wait for their content at once, and `POST /media` past that answers `503` with `media_upload_limit` until one is checked, deleted or expires.
- 89b7b38: Invalid compositions report every problem as a structured diagnostic, with a stable `code`, a JSON pointer `path`, the `node` id and a message, instead of one joined string. Errors are collected per phase (the document's shape, functions, nodes, connections, then the graphs), stopping at the first phase with errors, and at most 50 are reported. `lumen_engine::json::check` runs the same pass without rendering, and `json::parse` fails with a `ParseError` whose `diagnostics()` lists them. The first warning, `font.fallback`, says a text node's family resolved to no font: `check` raises it for literal families a host's font lookup can't resolve, and renderers raise it once per node and family when shaping falls back, through `GpuCompositionRenderer::take_diagnostics`. Warnings never fail a render, and clients must accept codes they don't know. The engine README lists the codes.

  `lumen-local --check` prints them and exits 1 when there are errors, with `--format json` for `{ "ok", "diagnostics" }`. `lumen-server` answers `POST /renders/check` with the same object, and the `422` for an invalid composition on `POST /renders` carries a `diagnostics` array. Messages lose their `composition structure validation failed:` prefix, and for Rust users, `json::unknown_param_error` is removed and `GraphValidationError::InvalidSettings` gains a `field` pointer.

- 04f41b2: With `LUMEN_HARDWARE_DECODE=1` on Linux, Vulkan hardware decode now covers every H.264, HEVC and AV1 profile the device's driver can decode, not only 8-bit 4:2:0. 10-bit HEVC from iPhones, 10-bit AV1 and 4:2:2 or 4:4:4 HEVC decode on the GPU where the hardware supports them; anything the device refuses still moves to CPU decode with one warning. For Rust users of `lumen-engine-ffmpeg`: `VulkanDecodedFrame::layout` returns a `VulkanFrameLayout` describing the frame's subsampling, depth and buffer layout for `copy_to_buffer`.

### Patch Changes

- 9208a20: For Rust users of `lumen-engine-ffmpeg`: `VideoDecoder::receive_rgba_frame` and `receive_cpu_frame` are removed in favour of `receive_video_frame`, and nothing in `lumen-ffmpeg` converts YUV to RGBA any more. YUV sources come out of `receive_video_frame` as planes with their colour tags, and the renderer's shader does the conversion; sources that are already RGB, gray or palette-based still come out as RGBA. `YuvVideoFrame::sample` reads the Y, Cb and Cr values at a pixel.
- e06506b: Open-GOP HEVC videos, such as x265's default output, showed the next frame instead of the one asked for when playback stepped backwards onto a frame just before a keyframe, on both CPU and hardware decode. A seek whose first decoded frame comes after its target now seeks again from an earlier keyframe.
- 1484e3a: Gradient paints with `interpolation: "linear_srgb"` now blend their stops in linear light, as documented. They were mixed in sRGB like the default, so mid-gradient colours came out darker than asked for. `srgb` gradients are unchanged.
- 92b28e3: Every package now has a README, so its npm page describes it.
- 5260e32: The preview hands video to the engine as the browser's native YUV planes, and the engine converts them with the same matrix as a server render, instead of leaving the conversion to the browser. Untagged video now looks the same in the preview and in a render: below 720 rows it's BT.601, and from 720 rows it's BT.709. Chrome used to pick BT.709 for sizes like 960×540 and 720×576, and Safari for everything, including video tagged BT.601. Video also shows in Safari again, which ignored the RGBA conversion and left frames black. Rotated video is rotated by the engine.
- 672e2f8: `POST /renders` on `lumen-server` resolves `quality` and `codec` when the render is requested, with the resolver the render uses, and returns `422 invalid_quality` for a preset the host doesn't define, a codec none of the encoders it tries has settings for, or a codec other than the one `LUMEN_VIDEO_ENCODER`'s encoder writes, instead of accepting the render and failing it later.
- 699ae65: A `path` or `shape` stroke under a pixel wide now covers at most its width of a pixel, so a stroke animated to zero width fades out instead of popping off on its last frame. Centred on a pixel, a 0.1 px stroke drew at about 55% alpha and now draws at 10%. Strokes 1 px wide and up are unchanged.
- b9efc3b: Server CPU video decode hands YUV planes and the stream's colour tags to the renderer instead of converting to RGBA in FFmpeg, so server video converts with the same GPU shader as every other path. Video with an alpha layer keeps it as a plane, and rotation is applied by the shader.
- 833d912: The engine can take video frames as YUV planes (NV12, I420, I422, I444, 8 to 16 bits, with an optional alpha plane and rotation) and convert them to RGBA with one GPU shader. Nothing produces these frames yet, so output is unchanged.
- 892990a: Hardware-decoded video converts to RGBA with the same YUV shader as every other decode path, following the stream's colour tags. VideoToolbox frames on macOS no longer render white, and rotated sources now decode on the GPU and show upright.
- Updated dependencies [4df3509]
- Updated dependencies [92b28e3]
- Updated dependencies [29f9e9b]
- Updated dependencies [bc958b9]
- Updated dependencies [8b9b483]
- Updated dependencies [5260e32]
  - @lumiscia/lumen-preview@0.5.0

## 0.4.0

### Minor Changes

- aa3f697: For Rust: audio is 48 kHz stereo by contract, so `AudioBuffer` loses its sample rate and takes channels of equal length (`from_channels` panics otherwise), `AudioMetadata` is gone, and `AudioResolver` only has `resolve_range`. `AudioMixer::mix_into`, `AudioBuffer::silent` and `AudioBuffer::channels_mut` are gone; use `mix_range`. Mixed audio is unchanged.
- 833b161: `lumen-bench-composition` now reports setup, first frame and steady-state fps separately, prints whether each row ran with static-output reuse disabled, and gains a `vulkan-video` mode that renders and encodes on one Vulkan device. The `*-profile` modes became `--profile`, and the `vk-cuda-export` mode and `vector_showcase` composition are removed; `--save` no longer overwrites one file when several encodes run. `lumen-bench-decode` drops its memory columns and fails if the crate and the ffmpeg CLI decode different amounts of video, and `lumen-bench-text` skips the emoji case when no loaded font covers it.
- b56766f: Compositions can no longer crash or hang the renderer through expressions or media graphs. Expressions nested more than 64 levels, or functions calling each other more than 64 levels deep, are parse errors instead of a stack overflow. Nodes that read each other's properties or text measurements fail with an error once the reads nest more than 96 levels of evaluation, counting each expression's depth, or start more than 10,000 evaluations. `clamp` with a NaN bound is an error instead of a panic, and frame prediction no longer takes exponential time on graphs that share inputs.
- 0a2034c: Native renders no longer read the machine's installed fonts, so text renders the same on every machine, as it already did in the browser. A text node's font family must be loaded through the media store, or it falls back to the bundled Roboto and any other loaded fonts: `lumen-local` resolves families from its font directories and the system, and `lumen-server` from its media and font roots. Characters a font lacks, such as CJK, Hebrew or emoji, fall back only to loaded fonts, chosen the same way on every platform. Load a font that covers them instead. `lumen-bench-text` takes `--font PATH` to load fallback fonts for its emoji and mixed-script cases.
- a57ee89: Native audio decodes inside the renderer, so `lumen-local` and `lumen-server` no longer need the `ffmpeg` executable on `PATH`, and a long audio track no longer keeps every decoded 30 s window in memory: a 20 minute two-clip render peaked at 1035 MiB and now peaks at 170 MiB. Audio that starts after the container's first timestamp, such as a video whose audio track begins half a second late, now reads as silence until it begins, where the executable played it from the start. The same holds for a gap in a stream's timestamps, which reads as silence instead of pulling later audio early. A source that ends before a clip does returns the samples it has, where it was padded with silence to the requested length. A damaged packet costs its own samples instead of failing the render, and a stream whose channel layout or sample rate changes midway keeps decoding. Clips that overlap on one source each keep a reader (up to four per source), so none of them seeks while it plays.

  For Rust: `lumen-engine` drops its `ffmpeg-next` dependency, and `lumen-engine-ffmpeg` gains `AudioRangeReader`, which reads 48 kHz stereo ranges of an audio stream.

- c14a62d: Removed the `alpha_premultiply` node. Every raster is now premultiplied, so there is no straight-alpha state for a composition to convert into, and the filters, `merge`, `raster_multimerge`, `boolean`, `blur`, `shadow`, `transform` and `wgsl_shader` no longer branch on one. A composition that still uses `alpha_premultiply` is rejected with a message.
- 3952302: Removed the `channel_shuffle` node. It could write a channel straight into a premultiplied raster, such as alpha forced to one under unchanged colour, which no other node can do. A composition that still uses it is rejected with a message; a `wgsl_shader` node rearranges channels when that is needed.
- 7a4579f: Removed the `curves` node. It ran the same table as `color_grade` with linear sampling, which is `color_grade`'s default `interpolation`, so renaming the node to `color_grade` and `curve_source` to `lut_source` renders the same pixels. A composition that still uses `curves` is rejected with that instruction.
- 162d015: Removed the unused `#[property(...)]` field attribute from `#[derive(Node)]` and the `#[node_impl]` attribute macro from `lumen-engine-macros`. No node used either, and the generated node definitions are unchanged. Node parameters are declared with `#[params]` and `#[derive(Delegate)]`.
- 3b84514: Removed the `memo` node. Static branches are already reused across frames without it, and a branch that depends on the frame is rendered every frame. A composition that still uses `memo` is rejected with that message. The snapshots it held, and the plan pinning and cross-plan copies that kept them, are gone with it.
- 6d8625f: Removed the `polygon` shape geometry and the `polygon_points` parameter of `shape`. Draw a polygon with a `path` node and closed path data, such as `M 0 0 L 12 0 L 0 12 Z`.
- 16d34f5: `lumen_server::service` is removed: the queue, executor, artifact store and progress sink traits and their in-memory, presigned-URL and callback implementations had no users. Embed `lumen_server::render` instead. The inert `legacy-decode-tests` feature is gone, and `reqwest` and `futures-util` are only built with the `cli` feature.
- e3ef24f: Removed render-plan features nothing used from `lumen-engine-gpu`: node ownership and parameter slots, indexed and indirect drawing, scissors, vertex and index buffers, the `GpuAdapterInfo` mirror (use `wgpu::AdapterInfo`, now optional for external devices), `ExportableVulkanTexture::row_pitch` and several discard-result helpers. Rendering output is unchanged.
- dc1e160: Node structs now derive `Default` instead of spelling it out, so a new node cannot get its default wrong. `PortRef::empty()` is replaced by `PortRef::default()`.
- b6ece6b: Composition errors now name the JSON path, such as `timeline.fps: ...`, and more mistakes fail instead of being ignored. Unknown keys in `timeline` and `render_settings` are errors, including in templates, so a typo like `fpss` no longer falls back to 30 fps. Other unknown keys at the root and in `audio` are still ignored.

  `render_settings.background_color`, which rendering never read, is an error: connect a `background` node instead. `Composition.setBackgroundColor` is gone. Audio clips need whole-number `start_ms` and `duration_ms`, and `source_start_ms` must be a whole number when set; `start_frame`, `duration_frames` and `source_start_seconds` are errors. `schemaVersion` is ignored and the schema no longer types it, `lumenSchemaVersion` must be absent or `0.1.0` in the schema too, and a non-string `$schema` or a `metadata` that isn't an object with a string `name` now fails at load, as it already did in the schema. The builder no longer writes `durationSeconds` into `timeline`. `CompositionBuilder::build` now rejects fps that isn't positive and finite, a zero size or duration, and a prop default of the wrong kind, for the root and templates.

- 6524812: An expression such as `title.opacity` now fails to parse with a message pointing at `node(id, 'property')`, instead of parsing and failing when it is evaluated. Reading a node property that has no expression value, such as a paint, now says `property `color` on node 1 can't be read in an expression`. Unknown node params no longer carry migration hints for the removed `wgsl_shader` `bindings` and `memo` `cache_id` params. For Rust embedders, `ExpressionContext::duration_frames`, `TrackId`, `ExpressionId`, `ExpressionReference`, the `resolve_*` property methods and the `expr` submodules are gone.
- 18a320a: Expressions no longer treat numeric strings as numbers. `'1' + 2`, `vec2('12.5', 2)` and a param set to `='12'` now fail with an error naming the string instead of converting it; a number still becomes a string for text content, and booleans still count as 0 and 1. Only exact zero is zero: `1 / sin(3.141592653589793)` is a large number rather than a division-by-zero failure, `mod` and `%` behave the same way, and a value below 2.2e-16 now counts as true. `text_width(node(id), 32)` is rejected when the composition loads instead of failing mid-render.

  For Rust: `Expression::new` is no longer public, and `lumen-bench-expression` times expression parsing and evaluation on their own.

- 0003560: Expressions fail earlier and accept more number forms. An unknown variable such as `=frmae + 1` is now rejected when the composition loads instead of failing mid-render, an unknown string escape such as `'\q'` is an error rather than dropping the backslash, and `node(id)` rejects ids above 2^53. Numbers may now use exponents and a leading dot (`1e3`, `1.5e-3`, `.5`), and a number too large to represent is rejected rather than becoming infinity. The SDK's helpers write large and small numbers in exponent form as a result.

  The `linear(a, b, t)` builtin and the SDK's `linear` helper are gone; use `lerp`. `'linear'` stays as an easing name. `x-lumen-expressions` in the schema now lists `forms` (`if` and `node`) and `reserved` names, which `@lumiscia/lumen-types` exports as `expressionForms` and `expressionReservedNames`.

- 3f6dc3e: Render errors now say what failed. An expression failure names the node, the param and the variable, as in `node 1 `content[1].font_size`: division by zero` and `node 1 `font_size`: undefined variable `foo``, and one reading another node's param names both. Graph errors name the nodes and ports they hold, and a cycle prints its path. Parse errors give the expression's source and the byte the parser stopped at. Messages no longer repeat themselves or carry an `expression error:` style prefix.

  For Rust: `ExpressionContext::path` and `NodeParamEvalContext` are gone, `ExpressionContext` is `Copy`, `DelegateEvalContext` holds only `node_id` and `expr`, `NodeParams` builds on `DelegateEvaluable`, an expression that returns the wrong type now fails with `ExpressionError` rather than `PropertyError::InvalidType`, and `RenderError` variants drop the `frame` field they never printed.

- c78a329: Evaluate repeated expression subtrees once per evaluation, reducing CPU work during timeline-bounds preparation and frame binding. Values are shared only within the same evaluation; short-circuit branches, error ordering and contextual inputs retain their behavior. In Rust, read the expression tree with `Expression::ast()` instead of its former public `ast` field. Construct a custom tree with `Expression::new(ast, source)`, which derives references and its evaluation plan together. TypeScript and JSON expression usage are unchanged.
- bb94902: For Rust users of `lumen-engine-ffmpeg`: the GPU encode telemetry types (`GpuEncodeTelemetry`, `GpuEncodeEvent`, `GpuUploadDescriptor` and `MuxedEncoder::gpu_telemetry`) are removed, since nothing read them and their upload timings were measured at the wrong point. `VideoEncoder`, `AudioEncoder` and `OutputContext` are no longer public, because `MuxedEncoder` is the only way to finish an encode, and `lumen_ffmpeg::encode::RateControl` is now `lumen_ffmpeg::RateControl`. `AudioFrame` and `AudioResamplerConfig` lose `sample_format` and `pts`, and `SampleFormat` is removed: audio is always interleaved f32 and is written as one contiguous stream from time zero. `MediaFrame::ExternalTexture` and its frame and handle types are gone from `lumen-engine`, and the `avcodec`, `avformat`, `swresample` and `swscale` features of `lumen-engine-ffmpeg` are always on.
- 17f7d59: Failed audio setup and failed file reads no longer leak FFmpeg codec contexts, resamplers or format contexts, and a failure to close the output file now fails the render instead of being ignored. On multi-GPU machines, `lumen-server` and `lumen-bench` now run NVENC on the GPU that rendered the frames, and CUDA interop frees its GPU resources in the right order and context, where it used to leak a mapped array on every NVENC render.

  For Rust: `CudaContext` and `CudaDeviceAllocation` no longer borrow a `CudaDriver`. Allocation, copy, synchronize and converter creation are methods on `CudaContext`, and `import_owned_vulkan_opaque_fd_image` takes a `&CudaContext`. `import_vulkan_opaque_fd_image`, `CudaExternalMemoryHandle`, `CudaExternalSemaphoreHandle` and `VulkanToCudaExport` are removed, and `VideoEncoderConfig` gains `cuda_ordinal`.

- ab3881b: For Rust: `path`, `shape` and `text` params group their transform in `placement`, and `path` and `shape` their fill and stroke in `style`, so `params.position` is now `params.placement.position`. Composition JSON and the schema stay flat.
- b4df9b5: The frame graph is now the only renderer. The compiled plan path and the `LUMEN_FRAME_GRAPH` flag are gone, so every frame is recorded with pooled textures and cached programs.

  Behaviour changes:

  - A shader that writes fewer pixels than it did on the previous frame no longer leaves the old pixels behind. What it doesn't write is transparent.
  - If a source's first output frame shows it twice and only the second import fails, the render fails with the "would change" error instead of moving both uses to CPU decoding.
  - Video jobs can hold more VRAM.
  - Shaders chosen by an expression build at first use. A bad literal shader still fails at compile time when the output can reach it.
  - `compile()` now checks every branch connected to the output, not only what frame 0 shows, even if no frame ever reaches it. A bad literal shader or uniform under a clip that hasn't started, a zero-duration clip, or any layer of an expression switch, including one like `selected_layer: "=0"`, fails at compile. Disconnected nodes, unplayed templates and the unselected layers of a literal switch still aren't checked.
  - `compile()` no longer evaluates uniform expressions, so one giving the wrong component count fails at the frame that uses it. Literal uniforms are still checked at compile, including the length of a vector with expression components.
  - `render_frame_into_external` accepts more targets: a `COPY_DST` texture of the right size and format no longer needs the other usages. A wrong size or format is still refused, with a reworded error.

  For the preview: the WASM `LumenPreviewController` no longer has `setLookaheadCount`, because there is nothing to build ahead. `setLookaheadCount` on the `@lumiscia/lumen-preview` controller and the `lookaheadCount` prop still set how far ahead media loads.

  For Rust: `lumen-engine-gpu` loses `RenderPlan`, `RenderPlanBuilder` and the plan resources, keeping `Recorder`. `GpuCompositionRenderer` loses `compile_with_media`, `precompile_frame*`, `set_plan_cache_budget`, `compiled` and the plan counters. `plan_cache_bytes` is now `held_bytes` and `cached_pass_count` is now `skipped_pass_count`. `RasterHandle` holds the texture itself, and `ExternalTexture` constructors no longer take a domain. `lumen-local`'s summary prints `held_bytes`, `skipped_passes` and `submitted_passes`.

- 179f80b: Gradients now reject what they used to ignore. A gradient with more than eight stops fails to load instead of silently dropping the highest offsets, and the schema caps `stops` at eight. A gradient with an unknown key, such as a misspelt `spred`, is an error instead of being skipped. In Rust, `Paint::to_gpu` no longer takes a fallback colour: a gradient without stops draws transparent.
- 3b7bec3: A connection's source is now just the node it comes from, since every node has one output. JSON `connections` read as before, and `from_port` may still be written but must be `output`; anything else is an error naming the connection. The composition schema and SDK types do not change. A node that is missing a required input, such as a `blur` with nothing connected to `source`, now fails with ``node 3 is missing required input `source` `` when it compiles as well as when it validates.

  For Rust: node inputs are `Option<NodeId>`, or `Vec<NodeId>` for variadic inputs. `PortRef`, `OutputPortDef`, `PortKind`, `SINGLE_RASTER_OUTPUT` and the `port` argument of `compile_gpu` are removed, and `Connection` loses `from_port`.

- b734d34: An expression that gives NaN or infinity for an enum param is now an error instead of selecting the first option. A failure to open a video encoder names the encoder, and `lumen-server` keeps up to 1,024 characters of an error message instead of 256, so the codec, stream and node details at the end of it reach the client. `RenderError` no longer has the variants nothing returned (`Cancelled`, `MissingNodeOutput`, `InvalidMediaOutputType`, `SurfaceAllocation`, `PixelReadbackFailed`, `ScratchSurfaceUnavailable`), and `CudaDriver::describe_error` is gone.
- ba5b6dd: `lumen-local` parses its command line with clap, so `--help` prints to stdout, a bad command line exits with code 2 and clap's message, and `--encoder` or `--bitrate 14M` with a `.png` output is an error even when the value matches the default. `LUMEN_VIDEO_ENCODER` and `LUMEN_FONT_DIR` still act as defaults for the flags. The `vulkan` feature of `lumen-local` is gone, because Vulkan is always built on Linux.
- 10fbe21: Media that can't be opened now says why. A corrupt image used to report `media source not found`, and `lumen-local` printed the real cause separately; the render error now carries it, as in `media decoding failed for hero.png: failed decoding image source: ...`. `lumen-local` also names the path it looked at for a missing file. `lumen-server` reports `media source `hero.png` not found: no such file in the media root` and doesn't say whether a file exists outside the root.

  For Rust: `MediaStore::get_image_resolver`, `get_video_resolver` and `get_audio_resolver` return `Result<Box<dyn _>, MediaError>` rather than `Option`, and default to `SourceNotFound`, so a store only implements the kinds it serves. `MediaError::SourceNotFound` gains `details`, and a file with no audio stream now fails with `MediaError::Decode`.

- aeca11d: Rendered output for video inputs changes intentionally: `lumen-local` and `lumen-server` now convert video with its tagged colour matrix and range, where they used to read everything as limited-range BT.601. Untagged video reads as BT.709 from 720 lines up and BT.601 below. Phone video with a rotation flag now renders upright at its display size, native and in the browser preview. Files whose first frame has a non-zero timestamp now show the right frames, and files with several video tracks use the one FFmpeg ranks best. `lumen-local` no longer runs out of memory on long clips, because each video source now caches at most 64 frames or 256 MiB.

  Breaking: a video with no usable frame rate or duration now fails to open with `unable to determine stream frame rate` or `unable to determine stream frame count`, where it used to get an invented frame rate or a one-frame length.

  For Rust: `VideoStreamInfo` gains `rotation`, `frame_rate` and `start_time`, its `width` and `height` are now display dimensions, and `InputContext::duration_us` is new.

- 2aa8525: A node's input fields are now the only record of its connections, so the checks that run before rendering read the same wiring the renderer follows. A second connection to an input that takes one source is now an error, ``input `base` of node 3 is already connected``, instead of replacing the first. Inputs that take several, such as `layers`, still append. JSON `connections` are unchanged.

  For Rust: `Graph` holds only `nodes`; `connections` and `outgoing_connection_count` are gone, and `Graph::connect` writes the connection into the target node.

- b4c8a05: Each node kind in `lumen-engine` is now declared on one line of a table in `node/kind.rs`, which generates the `NodeKind` enum, its dispatch, its schema list and JSON construction. For Rust: `NodeKind::id`, `input_ports` and `has_expressions` are now reached through the `Node` trait, which gains `has_expressions`, and `JsonNode` gains a `KIND` constant. The generated schema and composition JSON are unchanged.
- 8e634bc: Params evaluate through one `Deferred::eval` that holds the only literal-or-expression match, and each param type now only converts an evaluated `ExpressionValue`. Compositions render and fail as before. For Rust: `DeferredValue` is now `ParamValue`, `eval_deferred` is now `from_expression`, and `eval_number` and `eval_int` are replaced by `truncated_int`, which takes the evaluated value.
- 2ee11ec: `lumen-local` and `lumen-server` now resolve an encoder name through FFmpeg, so `hevc_nvenc` and `av1_nvenc` work in `lumen-local`, `libsvtav1` and other installed encoders work in `lumen-server`, and an unknown name fails before rendering starts. The `h264`, `hevc`, `h265` and `av1` aliases and the `*_vaapi` encoders, which never rendered, are now refused with a clear error. In `lumen_ffmpeg`, `VideoEncoderConfig` takes one `encoder` name and one `quality`, every video frame needs a `pts`, and `gpu_texture_encode_support` is gone in favour of `encoder_info`.
- 542f5a5: Documents parse each expression once, since `prop()` checks read the parsed params instead of parsing the JSON again. For Rust: `DelegateEvaluable` implementors provide `for_each_expression` rather than `has_expressions`, and `Expression` has `reads`, found once when it is built.
- aa9207c: `lumen-server` and `lumen-local` now find fonts the same way: by the family name inside each font file, in `LUMEN_FONT_DIR`, then `<media-root>/assets/fonts`, then the system fonts. `lumen-server` used to guess file names under `LUMEN_MEDIA_ROOT/fonts`, so it never read the system fonts and ignored `LUMEN_FONT_DIR`. `LUMEN_MEDIA_ROOT` no longer supplies fonts, and a font file is no longer found by its file name or path. A `--font-dir` or `LUMEN_FONT_DIR` that isn't a directory is an error at startup instead of silently using other fonts. A family that no font has now logs a warning once and renders in the bundled font, as before. An installed Roboto no longer replaces the bundled one, and the demo compositions drop their `font_family`.

  For Rust: `FontCatalog` moves from `lumen-local` to `lumen_server::render`, with `existing_font_dir` and `font_dir_from_env`.

- 9f9ca77: Params now parse JSON through one path, with no intermediate value type. Every composition that parsed before parses to the same params, and the generated schema is unchanged. Error text changes in two ways: nested params name one full path, such as ``node 1: `content[0].underline.thickness` expected float`` in place of ``node 1: `content`: [0]: `underline`: `thickness` expected float``, and an expression where a param takes literals, or an integer out of range, now says so directly, such as ``node 7: `width` must be an integer from 0 to 4294967295, got -1``.

  For Rust: `PropertyValue`, `PropertyExpression`, `DelegateValue`, `OptionalDelegate`, `DelegateVec`, `DeferredJsonValue` and `#[delegate(properties)]` are gone, and delegates no longer implement `Deserialize`. `DelegateEvaluable` gains `from_json_value`, `to_json_value` and `read`, and `ParamValue` gains `from_json_value`.

- c22514f: Node compilers in `lumen-engine` now return a `RasterHandle` directly, and rasters carry `alpha_mode` rather than a `RasterMetadata`. Removed from the Rust API: `CompiledOutput`, `RasterMetadata`, `ColorSpace`, `CompiledComposition::node_outputs`, `CompositionMetadata` and `Composition::metadata`, `TimelineSettings::time_seconds`, `BoundFrame::write_texture_rgba8` and `write_texture_rgba16_float`, `error::Warning`, and the error variants nothing raised. Compositions render the same, and a `metadata` key in composition JSON is still accepted and ignored.
- ff9a9da: `lumen-server` keeps `h264_vulkan` frames on the GPU, and `lumen-local` gains prefetch, text sessions and CUDA, because both now run one render loop. For Rust: `lumen_server::render` adds `Mp4Render`, and `LocalMediaStore::new` takes the media root and font directory.
- 64dbddf: For Rust: `CompositionBuilder` and `CompositionTemplate` hold their graph, timeline and render settings in a new `Scene`, as `scene`, and `Scope` holds `scene: &Scene`. `Composition::graph`, `timeline` and `render_settings` still work, and `Composition::scene` is new. Composition JSON and rendering don't change.
- 873ae1f: Measuring text and then drawing it now shapes it once: one cache of the 32 most recent shapes replaces the separate fit and measurement caches. Solid `background` paints no longer run their supersampling loop, and the text shader only reads the atlas in the branch that uses it. Rendered output and composition JSON are unchanged. For Rust: `TextSystem::shape` now returns `Arc<ShapedText>`, and `TextSystem::shape_changed` shapes text without keeping it.
- 20ae861: `node(id, 'param')`, default params in the schema and nested `to_json` now convert a param directly rather than through `PropertyValue`, and reading a param that holds an expression no longer clones that expression. Compositions and the generated schema are unchanged. For Rust: `get_property` returns `PropertyRead`, a borrow of the expression or the literal's value, `NodeSchemaDef::default_properties` holds JSON, and `ParamValue` gains `to_json_value` and `to_expression`.
- 85ec2a1: Malformed `path` data now fails the frame with an error naming the node and its `data` field, such as `` `data` is malformed: unexpected data at position 1 ``, instead of rendering nothing. Coordinates and radii beyond +/-1e7, and arcs too extreme to flatten, are refused the same way, and an empty string is still an empty path. Fill antialiasing now also covers the closing edge of an open path, so it matches the same path ending in `Z`, and a stroke width of zero or less draws no stroke.
- 0203382: Translucent images and video now render correctly in `lumen-local` and `lumen-server`: their pixels are premultiplied when decoded, as the browser preview already did, so PNGs, ProRes 4444 and other alpha video stop rendering too bright and stop leaking colour from transparent pixels. VP8 and VP9 WebM alpha is detected and decoded with libvpx, and renders opaque with a warning on FFmpeg builds without it. `wgsl_shader` inputs from these sources now hold premultiplied colour, as documented.
- f5b04fa: Preview errors are readable and precise. `preview.error` is now the message rather than a stack trace, with the stack still logged to the console. Media failures always name their source, through the message and the new `LumenMediaError.sourceId`. Render failures from the WASM bindings are now `Error` objects rather than strings, and a video frame that isn't loaded yet has `code: "media_frame_out_of_range"`. Only that code makes the preview load the frame and render again, so an unrelated error that happens to contain the words "media frame" and "out of range" is no longer retried.
- 4891f45: Preview audio now mixes like export and reads the timeline the engine parsed, stops as soon as a composition or media reload begins, and reports sources that fail to load instead of failing silently. Replaced or removed media is cancelled and can no longer overwrite newer sources. The package no longer ships its tests. Decoded frames and inputs no longer leak, and failed media loads can be retried. Breaking: `timelineMode`, `native-samples`, the `audioTimeline` session option, Mediabunny `Source` inputs and the audio schedule and parsing helpers are gone, and a video with no `fps` and no readable frame rate is now an error.
- 9494d2c: The preview now has one transport and one clock: a session always renders in a worker, on a canvas it creates inside a host element, so remounts and replaced compositions can't fail on a canvas that was already transferred.

  - `Player` renders a `<div>`, and `className`, `class` and `style` apply to it. The canvas fills it and keeps its aspect ratio.
  - `compositionJson` becomes `composition`, an object or anything with `toJSON()`. `mediaSources` and `audioSources` become one `media` map of alias to source, with kinds inferred from the composition. The new `fonts` map registers fonts by family.
  - `session.attach` takes the host element and never rejects. `mountPreview(host, options)` creates and attaches a session. Failures reach `error`, with a typed `errorCode`: `audio_failed`, `load_failed`, `render_failed`, `webgpu_unavailable` or `worker_failed`.
  - A session needs bindings that name a worker, so `bindings` is now `LumenWorkerBindings`. There is no main-thread driver and no fallback to one. `createLumenPreviewRuntime` stays for code that drives a controller directly.
  - `preview.seek` returns a promise: true once the frame is on screen or a newer seek's is, false if a load or failure got in first. `preview.controller` and `preview.reset()` are gone.
  - A failed load isn't retried until an input changes.
  - A composition too slow for its frame rate drops frames and still answers pause and seek.
  - `dispose` stops a worker still starting and settles `attach` at once, and options changed during startup apply once the worker is ready.
  - A playback holds for audio for about a second. If none has played by then, as on a page with no user gesture or with a mixer that sends nothing or sends late, video runs on elapsed time for the rest of that playback, and audio joins at that position when it arrives, without playing anything older. Once audio has played on time, it leads the clock and holds through an underrun, ramping back up when it returns. A source change more than a second into a playback spends the hold, so that change and any later underrun in the playback are silent gaps instead.
  - The wasm `LumenPreviewController` loses `play`, `pause`, `isPlaying`, `tick`, `targetFrameDurationMs` and `targetFrameForTimeMs`, and the runtime controller loses `tickAsync`. `renderNowAsync` and `setFrame` remain, and `prepareFrame` and `renderPrepared` are new.
  - In Svelte, read state from `preview.state`, a single reactive snapshot. `preview.frame`, `preview.isPlaying` and the other mirrored fields are gone. `PlayerProps` and `MediaRegistration` are exported from the root, and declarations are generated.

- 267b94c: The Rust `lumen-engine::Composition` is now read-only and fingerprints its content once when built. Construct it with `CompositionBuilder::new(graph, timeline, render_settings).build()`; structural validation now happens during `build()`, which returns `Result<Composition, Vec<LumenError>>`. Read its parts with `graph()`, `timeline()`, `render_settings()`, `metadata()`, `audio()` and `compositions()`. To edit, consume it with `into_builder()`, change the builder's public fields and call `build()` again. This replaces direct field access and `Composition::new`. TypeScript composition builders and the JSON format are unchanged.
- 38f2bd2: Removed CUDA hardware video decode and its `nvcc` build step. `LUMEN_HARDWARE_DECODE=1` decodes on the renderer's Vulkan device (FFmpeg 8.0+), and anything else, including NVDEC-only setups and older FFmpeg, decodes on the CPU with a warning. CUDA/NVENC encode is unchanged, and the `cuda` feature no longer needs the CUDA toolkit to build.
- cb03d75: Removes preview exports nothing used. `createLumenPreviewRuntime` now returns only `LumenPreviewController`, and `LumenMediaStore`, `LumenRenderer` and `registerFontFamilyBytes` (use `registerFontFamily`) are gone from `@lumiscia/lumen-preview` and `@lumiscia/lumen-bindings`. The controller drops `togglePlay` (use `play` and `pause`), `frameRequirementsWindow`, `snapshot`, `setCompositionSyncMs` and `recordRenderTiming`. The media helpers, such as `LumenMediaBridge`, `fetchBlob` and `normalizeFps`, are imported from `@lumiscia/lumen-preview/media` instead of the package root.
- 05dd5d4: `lumen-engine` no longer has `json`, `msdf` or `webgl` Cargo features: JSON parsing and MSDF text are always compiled in, so `--features json` and `--features msdf` are no longer needed or accepted. The engine's `LogLevel`, `set_log_level` and `log_level_enabled` are gone; the browser build now sets its console level itself, and `setLogLevel` accepts only `off`, `error`, `warn`, `info`, `debug` and `trace`.
- 5299e06: Several render node fixes. A `crop` now keeps its bounds through `exposure`, `levels`, `hue_saturation`, `opacity`, `color_grade` and the other per-pixel nodes, so a `resize` after them scales the cropped area instead of the whole canvas. `lumen-local` writes PNG files with straight alpha instead of premultiplied colour. A custom `wgsl_shader` must declare `@workgroup_size(8, 8, 1)`, and a `transform` with a pivot but without `custom_pivot: true` is now an error rather than a pivot that is sometimes ignored. `time_remap` keeps frames past 2^32 instead of wrapping to 0, and rejects a non-finite `frame`. Graphs built in Rust with inputs set directly on nodes are now checked for cycles, and a plan prepared on the GPU renderer can no longer be confused with another plan of the same shape.
- 7f55481: The composition schema no longer rejects numbers outside a param's slider range, such as `opacity: 2`; the engine clamps them as it always has, and the ranges stay in `x-lumen-constraints`. The schema now also rejects negative node IDs and audio clips without a start and a duration, which the engine already refused. A node whose `params` isn't an object fails with the type it got.
- b79d8ae: `lumen-server` is now safe to expose behind a token. Compositions can only read media from their manifest, downloads refuse non-public addresses on every connection (allow internal hosts with `--allow-media-host`), the token is checked on every route but `/health` before the body is read, and binding a non-loopback address without a token is refused at startup.

  Other breaking changes: `job_id` and the `project` alias are gone, `webhookUrl` returns `400 webhooks_unsupported`, invalid compositions return `422` before anything is downloaded, renders run one at a time by default (`--max-concurrent-renders`), only the latest 16 finished renders are kept (`--retained-renders`), and manifests are capped at 256 entries and 2 GiB per job.

  Every native host now reads video and audio only from containers that keep their data inside the file, and only from local files. This includes `lumen-local` and anything using `lumen-engine-ffmpeg`. HLS, DASH, concat lists, image sequences and SDP inputs are refused, because a staged playlist could otherwise make FFmpeg read files outside the job directory. MP4, MOV, WebM, MKV, MPEG-TS and the common image and audio formats are unaffected.

- 3bea82b: `lumen-server` errors now use the shape the SDK reads, `{ "code", "error" }`, so `client.render()` and `getRender()` report `render_not_found` and `invalid_project_payload` rather than `http_404` and a status line. Malformed bodies and bodies over the 2 MiB limit answer with this shape too, with the codes `invalid_request_body` and `request_too_large`. A failed render keeps its real code and `retryable`: `GET /renders/:id` and `/progress` return them as `render.error`, `progress` gains `errorCode` and `retryable`, and the `render.failed` socket event no longer always says `render_failed`.
- a98129c: `lumen_server::render::render_project_mp4` takes a `&Composition`, and `parse_composition` replaces `convert_project_payload`, `ProjectBundle` and `ProjectInfo`. `RenderOptions::verbose_debug` and `--verbose-debug` are gone; use `RUST_LOG=lumen_server=debug`. A server build without the `cuda` feature renders `*_nvenc` encoders through CPU upload instead of failing, and the synchronous render API no longer needs a Tokio runtime.
- 27be320: `lumen-server` keeps one record per render, so polling, `GET /renders/:id/progress` and the socket always agree. `GET /renders/:id/artifact` answers `409` (`artifact_not_ready`, or the render's own error code) instead of `404` until the render succeeds, sockets close after the final event, `render.status` reports `processing` while a render runs, `render.outputUrl` carries the artifact URL, `createdAt` is RFC 3339, and `progress.updatedAt` and `progress.resolution` are gone. `--progress-min-delta` rejects values outside 0 to 1.
- 845c8e0: Text enum params now take their names from expressions, so `font_style: "=if(frame > 10, 'italic', 'normal')"` works as the TypeScript types say instead of failing mid-render; numbers keep working. `background` `width` and `height` reject expressions at load, since they were only read once at compile and animating them did nothing; the TypeScript types now say `number`. A span with no text no longer changes where other spans wrap, and `font_size` documents how a ligature across two spans takes its size.
- 43c8b55: Shape `width` and `height` are now floats, so an animated or measured size no longer snaps to whole pixels. Whole-number sizes render exactly as before; a composition whose expression gives a fractional size, such as `=text_width("Open editor", 20) + 48`, now draws that size instead of truncating it. Sizes between 0 and 1 draw one pixel across.

  Int, uint and u8 params still truncate fractional expression results toward zero, but NaN and infinity are now a property error instead of silently becoming 0 or a saturated value.

- 37823f6: `@lumiscia/lumen-bindings` ships one wasm build instead of four.

  - `.` is now `/browser`, which works without a bundler and takes optional `previewWasmUrl` and `previewWorkerUrl` overrides. A pre-bundled preview worker ships at `/browser/worker.js`.
  - `/bundler` stays as the Vite entry and needs no configuration. Remove `vite-plugin-wasm`, `vite-plugin-top-level-await` and any `optimizeDeps.exclude` for these packages.
  - `/node` loads the same wasm with `initSync`. It parses compositions; rendering still needs a browser.
  - `/web`, `/nodejs` and `/no-modules` are gone. Use `/browser`, `/node`, or `initSync` from `/browser`.
  - `createLumenBindings` returns a plain object, and the `LumenBindings` class, its `target` field, `LumenBindingsLike` and `LumenPreviewBindingsModule` are gone. Non-URL wasm inputs go through `initSync`.
  - `createLumenPreviewWorkerHost` no longer takes `previewWasmUrl`, because the session sends the worker the URL from its bindings.
  - `preview()` shares one compile across overlapping calls, and an instance already installed stays, including one from `initSync`, so a late init can no longer invalidate live controllers. The one exception: on Chromium's main thread, a wasm over 8 MB installs asynchronously, and it replaces an instance that `initSync` installed from a different, smaller wasm in the meantime.

- fb4c25b: Compositions now fail to load instead of silently rendering something else. These are errors: `params` that isn't an object, unknown keys on nodes and connections (such as `properties` for `params`), duplicate node ids within a graph, repeated audio track or clip ids, audio clips on a track the timeline doesn't declare, and non-finite or negative audio gains. The audio checks also apply to compositions built in Rust. `lumen-server` and WASM parse errors now include the nested cause, such as the bad param inside a template, not just `composition `card``. `AudioTrack.addClip` now gives clips without an id the default `<track id>-clip-<n>`, so clips on different tracks don't collide.

  Media now renders correctly. A canvas-sized `media_in` wired straight to `media_output` writes to external targets, where GPU encodes previously got an empty frame. Media that changes size renders at its own size instead of stretched into the old one. Replacing an image in a `LumenMediaStore` with one of the same size shows the new pixels. Cycling one `media_in` through several same-size images uploads each once instead of on every switch; images no plan is showing are freed, least recently used first, once they exceed 256 MiB.

  For Rust: `MediaTextureKey` loses `frame`, `MediaTextureUpload::key` and `BoundFrame::use_media_texture` take `Option<MediaTextureKey>`, with `None` for video frames, and `GraphValidationError::InvalidAudio` and `MediaError::UnknownAudioTrack` are new. `texture_from_metal` takes an optional drop callback, and `GpuCompositionRenderer` gains `set_media_texture_cache_budget`.

- eb354e6: The text node draws underlines, strikethroughs and highlights. The new node-level `underline`, `strikethrough` and `highlight` params are off when absent and apply to string content and to every span that leaves its own unset. A span's own decoration replaces the node's, and `null` turns it off for that span.

  Consecutive spans that take the node's decoration form one run, which draws one line or box per line of text, so mixed font sizes don't step or seam. A line's absent `thickness` and `offset` come from the tallest span on that line, and its absent `paint` is the text's own paint. Highlights break per line like CSS `box-decoration-break: clone`: each line gets its own padding and corner radius. `extent` maps `0..1` along the run's text in reading order, continuing across line breaks and following right-to-left text, so `"extent": "=vec2(0, clamp(time * 2, 0, 1))"` draws an underline in across wrapped lines. Lines can be `solid`, `double`, `dotted`, `dashed` or `wavy`.

  Decorations are painted in CSS order (highlights, underlines, glyphs, then strikethroughs). They follow the node's transform, opacity and motion blur but not the `glyph_*` params. They never change the text's layout or `text_width` and `text_height`, and they count toward the text node's glyph limit. Text without decorations renders as before.

  For Rust: `TextParams` has `underline`, `strikethrough` and `highlight` fields. `GpuGlyphInstance` has a `GLYPH_MODE_RECT` render mode for decoration rects, made with `GpuGlyphInstance::rect`, and `ShapedText::decorate` lays out a `TextDecoration`. Node params can now be `Option<T>`, unset unless given, and their specs then have no default value. The generated `NodeParamSpec.defaultValue` is optional.

- 2ce557e: Text underline, strikethrough and highlight `extent` now measure fractions of the text's width, as documented, not of its bytes. In right-to-left text they run in reading order, with an ellipsis last. Animated reveals of text with non-ASCII or mixed-width characters, such as `"extent": "=vec2(0, frame / 30)"` over "Mi casa — 你好", now sweep at an even pace instead of speeding up over wide characters, and an ellipsis counts as part of the width. A text node that draws more than 2048 glyphs, counting a stroked glyph twice and each decoration once, now fails to render with an error naming the node and the count, instead of silently dropping the rest and disabling cached output for the whole composition. Whitespace is not drawn, so it no longer counts. Text whose layout and decorations are constant allocates and draws only the instances it needs. Glyph-atlas entries for whitespace and glyphs that cannot use a distance field are released once unused, and the overfull-atlas retry check follows the glyphs the atlas will actually place. For Rust: `TextSystem::prepare_glyphs` and `prepare_stroked_glyphs` no longer take `max_glyphs`.
- cd2ea3e: For Rust: `text` params group into `layout`, `decorations`, `draw` and `glyph`, spans into `layout` and `style`, and evaluated `TextContent` is always a list of spans holding what shapes the text, a string being one span. Composition JSON and the schema stay flat, and rendering does not change.
- 4f3894f: The text node takes a `font_features` param, and spans take the same field: OpenType features by four-character tag, such as `{ "tnum": 1, "liga": 0, "ss01": 1 }`. A value of `0` turns a feature off, `1` turns it on and larger values select an alternate. A span's features merge over the node's: the span's keys win and the rest are inherited. Tags must be exactly 4 printable ASCII characters and values non-negative integers, and errors name the field. Each value takes a literal or an `=expression`, but the whole map cannot be an expression.

  Features shape the text, so `text_width`, `text_height` and `text_bounds` follow them. A word takes the features of the first span that starts it, because cosmic-text shapes the spans of a word together. Text without features renders as before.

  For Rust: `lumen_text::TextSpan` has a `font_features` field of `FontFeature` settings, and the new `FontFeatures` param kind is generated as `FontFeatures` in `@lumiscia/lumen-types`.

- 93fb454: The text node can limit its text to a number of lines or a box, and shrink text to fit. The new `max_lines` and `max_height` params drop the lines past them, and are unlimited when absent. `overflow` sets how the last line shown ends: `clip`, the default, leaves it as it is, and `ellipsis` ends it with `…`. Clusters at that line's end, and whitespace before the ellipsis, are dropped until it fits `max_width`. The ellipsis goes at the line's end in reading order, so it sits on the left of right-to-left text, and it takes the style, paint and decorations of the text it follows.

  `fit: "shrink"` scales every font size, the node's and each span's, by one factor of at most 1, so the text fits `max_width`, `max_lines` and `max_height` without breaking a word. Letter spacing, a pixel `line_height`, and decoration thickness, offset, padding and radius scale with it. The factor is the largest that fits, down to the one that takes `font_size` to `min_font_size` (8 by default); lines that still don't fit there are dropped by `overflow`. A fit takes at most 12 shapes, and is kept for the text's layout, so measuring, bounding and drawing the same text share one fit, and constant text pays for it once.

  `text_width`, `text_height`, bounded output and decorations all cover the text as drawn, after truncation and fit. The `glyph_*` params count only the glyphs and lines shown. Glyphs keep their numbers, and the ellipsis is one glyph numbered after the last. Text that sets none of these params lays out as before.

  For Rust: `TextParams` has `max_lines`, `max_height`, `overflow`, `fit` and `min_font_size` fields. `TextLayoutRequest` has `max_lines`, `max_height`, `overflow` and `min_fit_scale`, and `ShapedText::fit_scale` gives the factor a fit chose.

- 5a0939c: Add the `text_content`, `text_line` and `text_highlight` param kinds for rich text. No node uses them yet. Text content is a string or an array of spans. Each span needs `text` and can override the node's font family, size, weight, style, letter spacing and color, and can set an underline, strikethrough or highlight; `null` turns off a decoration the span would otherwise inherit. Span fields, including enums, accept `=expressions` like node params, and evaluation errors name their path, such as `content[1].font_weight`. The composition schema describes these kinds in `$defs` (`textContent`, `textSpan`, `textLine`, `textHighlight`, `textLineStyle`), which are generated from the Rust param definitions. `@lumiscia/lumen-types` exports matching `TextContent`, `TextSpan`, `TextLine`, `TextHighlight` and `TextLineStyle` types.

  For Rust: nested `Delegate` structs can now have `Option<T>` fields, which are left out when absent, and `Override<T>` fields, which inherit, clear or set a value. A struct delegate marked `#[delegate(properties)]` parses its nested JSON field by field the way node params do, rejects unknown fields, and has a `to_json` method; other struct delegates still deserialize through serde. `PropertyDef` has a new `nullable` field. `TextFontStyle` accepts expressions, which also lets the text node's `font_style` take one.

- d792b38: For Rust users of `lumen-engine-text`: `TextLayoutRequest` now describes shaping only. `origin`, `color` and `disable_hinting` are gone, along with `TextSystem::layout` and `TextGlyph::color`; use `system.shape(&request).place(origin)`. `prepare_stroked_glyphs` is now `prepare_glyphs`, `PreparedGlyphs::frame` and `compacted`, `TextLayout::measurement`, and `GlyphAtlas::len` and `is_empty` are removed, and `TextAlign::Justified` is now `Justify`. Rendering and composition JSON are unchanged.
- 37500c3: The text node's `content` takes styled spans as well as a string. Each span's font family, size, weight, style, letter spacing and color override the node's, and anything a span leaves out comes from the node. Span text and every span field can be an `=expression`. String content, including `=expressions`, renders as before. Span decorations parse but are not drawn yet.

  Each span is drawn in its own `color` paint. The new `paint_space` param (`span` by default, or `text`) sets whether a span's own gradient covers just that span or the whole text. The node's `color`, which spans without a color use, always covers the whole text. `paint_supersample` still applies to the whole node.

  The `glyph_*` params can read the new `span` and `span_count` expression globals, so they can target a single span, as in `=if(span == 1, vec2(0, -10 * sin(time * 6 + glyph)), vec2(0, 0))`. `@lumiscia/lumen-shared` exports them as `span` and `spanCount`, and a text node's `content` takes a string `Expr`. `text_width(node(id))` and `text_height(node(id))` measure the spans, and `node(id, "content")` reads span content as its joined text when that text is literal.

  For Rust: `TextParams::content` is now a `TextContent`, and `TextParamsDelegate::content` is now a `TextContentDelegate`, so wrap string content in `TextContentDelegate::Text`. `GpuGlyphInstance` packs a paint index into the upper bits of `mode` (read and set with `render_mode()`, `paint()` and `set_paint()`), and `GpuTextGlobals` no longer has `paint_bounds`, since each paint now carries its own bounds.

- f35c132: The text node can outline its glyphs. The new node-level `stroke` param is off when absent and applies to string content and to every span that leaves its own unset. A span's own `stroke` replaces the node's, and `null` turns it off for that span. A stroke has a `paint`, a `width` in pixels and an `align`, and each field takes a literal or an `=expression`:

  ```json
  { "stroke": { "paint": [0, 0, 0, 255], "width": "=4 + 2 * sin(time * 6)", "align": "outside" } }
  ```

  `outside`, the default, draws the outline behind every glyph's fill, reaching `width` pixels past the glyph's edge, as video captions do. The fill covers the outline's inner half. `center` draws the outline over the fills, centred on the glyph's edge, like `-webkit-text-stroke`. Joins are round. The stroke's paint respects `paint_space` the way a span's `color` does. Strokes follow their glyph's `glyph_*` transform and fade, and the node's transform, opacity and motion blur. Decorations are not stroked.

  Distance-field glyphs draw their stroke from a true distance field in the text shader, at the fill's resolution, whose distance range doubles as often as the stroke needs, so animated widths stay smooth and share fields. Raster glyphs draw a stroked outline, keyed by its half width rounded to a quarter pixel up to 4 px and otherwise to a sixteenth of its power of two, so animated widths reuse them. A raster stroke too large for the glyph atlas is drawn from a distance field instead, and a stroke whose field is too large for the atlas is dropped. In `auto` mode each glyph's stroke takes the path its glyph does. `glyph_opacity` fades a glyph's stroke and fill as one, so a faded glyph's fill still hides the stroke under it. Neighbouring glyphs fade separately, so where one's stroke overlaps another both show. Shrink-to-fit scales stroke widths with the text. Strokes count toward the text node's glyph limit, and `text_bounds` and motion blur cover them. They don't change the layout or `text_width` and `text_height`. Colour and bitmap glyphs are not stroked. Text without a stroke renders as before.

  For Rust: `TextParams` has a `stroke` field and `TextSpan` a `stroke` override, both of the new `TextStroke` kind. `TextSystem::prepare_stroked_glyphs` takes a half width per span and returns the stroke instances in `PreparedGlyphs::strokes`, each outlining the glyph instance `PreparedGlyphs::stroke_glyphs` names. `GpuGlyphInstance::color` is replaced by `opacity`, since the shader only read its alpha, and a `partner` index used by the new `GLYPH_PARTNER` flag.

- 0d47151: The text node gains `text_transform`, `paragraph_spacing` and, on spans, `baseline_shift`.

  `text_transform` is `none`, `uppercase`, `lowercase` or `capitalize`, on the node and on spans, where an unset span takes the node's. It maps the text before shaping with Unicode case mapping, so `ß` becomes `SS`. `capitalize` uppercases the first letter or digit of each word, and a word continues across spans. Glyph, word and line numbering, `span` indices, `text_width` and `text_height` count the transformed text, as do decoration extents, which are fractions of its bytes.

  `baseline_shift` is a span's offset in pixels, positive up. It moves the span's glyphs, its own underline, strikethrough and highlight, and its gradient box, while the node's decorations that the span inherits stay on the line. The line keeps its height, so shifted text can overlap its neighbours, unlike CSS `vertical-align` with a length. `text_bounds` includes the shift, and `text_width` and `text_height` do not. Superscripts are a smaller `font_size` with a `baseline_shift`.

  `paragraph_spacing` adds pixels after each paragraph, which a hard line break starts, but not after the last. Wrapped lines of one paragraph keep their line pitch. It adds to `text_height`.

  Text that sets none of these renders as before.

  For Rust: `lumen_text::TextSpan` has `transform` and `baseline_shift`, `TextLayoutRequest` has `paragraph_spacing`, `TextSegment` has `shift`, and `TextDecoration` has `shifted`. `ShapedText::bounds` can start above the first line when a span is raised. The generated type for the `text_transform` param is `TextTransform`, and a node param named after its node is no longer prefixed with the node kind a second time.

- f3baf03: `media_in` video ranges now do what they say. `range_end` is the first frame past the range and is optional, so leaving it out plays to the end of the source, and `range_start` is honoured on its own. Before, `range_end: 0` meant "no range" and ignored `range_start`. An expression for `kind` or `loop_mode` that doesn't name a value, such as `=2` for `kind`, now fails instead of becoming `image` or `clamp`. A `media_in` whose source can't be found or opened fails when the composition is compiled, with the reason, instead of at the first frame.
- e3d02e6: `boolean` `operation`, `raster_multimerge` `blend_mode` and `color_grade` `interpolation` are now real enums, so a value an enum doesn't have is an error instead of falling back to the first option. `color_grade` `interpolation` takes `"nearest"` or `"linear"` instead of `0` or `1`, and `shadow` `color` fails with a clear message when it is a gradient, which it used to replace with the gradient's first stop. `merge` `blend_mode` and the `sampling` of `resize` and `transform` now also accept expressions.
- ca87eaa: `@lumiscia/lumen-types` now generates every node and composition type from `composition.schema.json`, so node params carry their schema descriptions as JSDoc and the root types can no longer drift from the schema. Enums that several nodes share are one type (`BlendMode`, `LoopMode`, `MaskMode`, `ChannelSelector`, `MediaInSourceKind`) and the old per-node names are aliases of it. New exports are `AudioClip`, `AudioTrack`, `RenderSettings` and `Timeline`. `MetaManifest` is gone, and `Connection`, `CompositionTemplate` and `LumenComposition` are type aliases rather than interfaces.

  The composition schema defines `timeline` and `render_settings` once in `$defs` and gives `metadata`, `$schema` and `lumenSchemaVersion` root types: a non-string version or `$schema`, or a `metadata` that is not an object with a string `name`, is now rejected. The `fps` default of 30 moves from the schema to its description. Consumers that vendor the schema need the new copy.

- fc80354: Vulkan hardware decode no longer exhausts the file limit on footage that seeks: on NVIDIA, a job on seeking H.264 held about 1,390 file descriptors and now holds about the same as one that doesn't seek. A source with a GPU frame already on a texture no longer moves to the CPU mid-render, which changed its pixels from that frame on; the render now fails with an error that says so. `lumen-server` and `lumen-local` raise their open file limit to the hard limit at startup.
- 8aed87e: `LUMEN_HARDWARE_DECODE=1` now decodes on Vulkan in `lumen-local` and `lumen-server` on Linux. H.264, HEVC and AV1 at 8-bit 4:2:0 decode through FFmpeg 8.0 or newer on the renderer's own device, and a compute pass converts them to RGBA without leaving the GPU. Everything else moves to CPU decode with one warning per source: older FFmpeg, VP9, 10-bit, sources with alpha, rotated sources and hardware without the codec. A frame that decodes but can't be imported into the renderer now does the same, where it used to fail the render.

  For Rust: `FfmpegResolverOptions` is no longer `Copy` and has a `vulkan_device` field on Linux Vulkan builds. `GpuCompositionRenderer::new_vulkan_decode` creates a renderer on a device with Vulkan decode, and `GpuVideoFrame` has a `Vulkan` variant.

### Patch Changes

- 273309f: `av1_vulkan` output now keeps the requested size in players that honour container cropping. AV1 has no crop window, so hardware that pads frames to its coded alignment, such as AMD VCN 4, turned 1920×1080 into 1920×1088 and 1080×1920 into 1088×1920. The stream now declares the coded size and the MP4 crops back to the requested one with a `clap` box. `ffmpeg` decodes these files at the requested size. Chromium ignores the crop and still shows the padding, and `ffprobe` reports the coded height with `crop_bottom` alongside it. Canvases the hardware codes exactly, such as 1280×720, 1920×1088, 2560×1440 and 3840×2160, are unchanged. The crop needs FFmpeg 7.1 or newer.

  `lumen-local` and `lumen-server` warn when an encode relies on this crop, and name the coded size to render at instead. `lumen-server` now accepts `av1_vulkan` through `LUMEN_VIDEO_ENCODER`, which it previously rejected as unsupported.

  For Rust: `MuxedEncoder::container_crop` and `VideoEncoder::container_crop` return the new `ContainerCrop`, which holds the coded and requested sizes, when an encode relies on the crop.

- be86db3: `lumen-server` no longer crashes with several renders running at once on NVIDIA. Renders that would pass the driver's limit on Vulkan contexts wait for a running render to finish.
- 83233e1: Video decoded on the CPU no longer leaves the last columns transparent when the width isn't a multiple of 8. A 854×480 source lost its last 6 columns and a 642-wide one lost 2; 1920×1080 and 1280×720 were unaffected. This affected every host that decodes video with FFmpeg on x86.
- 125daef: `h264_nvenc` on a machine with no CUDA library now falls back to CPU upload instead of panicking. `CudaDriver::load` returns an error when the library isn't installed.
- 7522c18: Fixed `lumen-server` renders on the CUDA/NVENC path (`h264_nvenc`, `hevc_nvenc`, `av1_nvenc`) containing frames from the wrong point in time and differing from run to run. The MP4 container of those renders now says BT.601, the matrix NVENC converts RGB with, instead of BT.709.
- 912d9e8: A render that fails before its first frame reaches a Vulkan encoder now reports its error instead of crashing the process. `lumen-server` crashed on this with `h264_vulkan`, its Linux default, and took every concurrent job with it, for example on a composition whose video `source` isn't staged.
- 4f256eb: FFmpeg error messages now name the GPU backend, codec and stream they came from. CUDA copy and encode failures on `lumen-server` name the frame, and a video encoder that fails to open is reported as not retryable.
- 3511931: On Linux, `lumen-local` and `lumen-server` now default to `h264_vulkan` and fall back to `libx264` with one warning when it won't open, while an encoder you name fails instead of falling back. `LUMEN_HARDWARE_DECODE=1` now moves a source to CPU decode with one warning when its hardware decoder fails, instead of failing the render, and sources with alpha always decode on the CPU.
- 306bbcd: `h264_vulkan` renders no longer hang on NVIDIA. The encoder could stall forever at a keyframe, which stopped `lumen-local` mid-render. It now encodes one frame at a time, which adds about 1.3–2.2 ms per 1080p frame, or 20–65% on cheap compositions. B-frames, file size and quality are unchanged.
- 8b8c211: `lumen-server` built with `cuda` and `vulkan` no longer uses hardware video decode by default, because it draws 640x360 and 1920x1080 video in the wrong colours. Set `LUMEN_HARDWARE_DECODE=1` to turn it back on.
- b07097e: Seeking in HEVC with B-frames on a hardware decoder no longer returns a frame from before the seek. FFmpeg's HEVC decoder keeps its output queue through a flush when it has no frame threads, so a time remap that stepped back just before an IDR frame drew a later frame instead of the one it asked for. CPU decode had the same bug on a host with one CPU. The same frames now come out on either path.

  For Rust: `VideoDecoder::flush` now returns a `Result`.

- ec44453: `kind: "video"` reads normal `.jpg` and `.jpeg` files again, and BMPs with non-zero reserved bytes. The input allowlist refused them because FFmpeg's probe picks the `image2` demuxer for those files. `InputContext::open` now allows `image2` for those extensions with filename patterns turned off, so `frame%d.jpg` can't read `frame1.jpg`. PNG, WebP, BMP, TIFF, APNG, and small JPEGs read as video open again too, by counting their frames from the file instead of failing with `unable to determine stream frame count`. `lumen-local` also renders `.mp4` output to a unique temporary file beside the target, so a failed render leaves no partial file and keeps any earlier output. It now refuses an output path that exists and is not a regular file, such as a FIFO.
- a921065: `color_grade` now fails with a clear error when LUT text holds `nan`, `inf` or a number too large for a float, instead of sending NaN to the GPU or silently clamping infinity. Finite LUTs render the same.
- 4b074a1: Every place that reads a leading `=` or a hex colour now shares one implementation. A `color` prop on a composition template no longer accepts `#+f+f+f`, which only that path let through; it is now an error, as it already was everywhere else. Other input parses as before.
- bc62189: Native video sources open their decoder once instead of twice, so a source opens faster and the reported decode mode is the one the decoder actually uses. Rendered frames are unchanged.
- 43e4714: Zero-copy `h264_vulkan` renders on NVIDIA run 19-32% faster on stock FFmpeg 8.0. The shared Vulkan device now lists one video encode queue, which also stops the encoder stalling at GOP boundaries, so the encoder no longer needs to be limited to one frame in flight. `h264_vulkan` encodes of CPU frames still use one frame in flight.
- 05d99fa: Each `lumen-server` job and each WASM media store now keeps its own fonts, so two jobs can use different files under one family name, one after the other or at once. Before, the first file loaded for a name was used by every later job in the process.

  Every face a job loads stays available for glyph fallback, the bundled Roboto included, and when faces tie for a request the most recently loaded wins. Loading a family again replaces what was loaded under that name. Removing a font or clearing a WASM media store now stops text using its fonts and frees them. A font file with no readable face fails the render, naming the family, instead of silently drawing fallback glyphs. A file whose faces go by another name logs a warning. `text_width` and `text_height` default to Roboto rather than whichever font was loaded first. `lumen-server` looks for a missing font once per job rather than on every frame.

  For Rust: `MediaStore::text_session` returns the store's new `TextSession`. A store that serves its own fonts should return one, as without it fonts load into a session shared across the process, and call `TextSession::remove_family` or `TextSession::reset` when it stops serving them.

- bf98386: `lumen-bench-json-parse` no longer has an `antialiasing_worst_cases_noaa` fixture; `antialiasing_worst_cases_aa` is the remaining antialiasing-worst-cases fixture. The `crates/local/demo` folder drops the duplicated no-AA, conic-angle and gradient-text scenes, and `feature-showcase.json` no longer carries an unconnected `boolean` node.
- 267b94c: Reduce render preparation and per-frame CPU overhead by recording consecutive GPU compute operations in shared passes, avoiding repeated text-atlas lookups and using faster hashes for binding maps and content fingerprints. Load all literal font families before recording the font generation in the compiled-plan key, preventing a redundant plan compile on the first text frame. These changes preserve the composition format and rendered output.
- 4ea40cd: `lumen-generators` renders the composition schema straight from the node definitions instead of through a second copy of their metadata. The generated schema is unchanged. The binary now takes the output directory as its only argument, and the crate exposes `generate` and `render_composition_schema_json` in place of its argument parser and manifest types.
- 22fe563: `lumen_gpu::Renderer::read_texture_rgba8` reads an RGBA8 texture back to the CPU with tightly packed rows, so hosts and tests no longer carry their own copy of the padded-row readback. Rendered pixels are unchanged.
- Updated dependencies [b4df9b5]
- Updated dependencies [aeca11d]
- Updated dependencies [f5b04fa]
- Updated dependencies [4891f45]
- Updated dependencies [9494d2c]
- Updated dependencies [cb03d75]
- Updated dependencies [37823f6]
  - @lumiscia/lumen-preview@0.4.0

## 0.3.1

### Patch Changes

- ea6e7da: Editing a node's params in place and rendering again with the same renderer now draws the edit instead of reusing the plan compiled before it. Any change to the composition, whether a literal param, an expression source, a connection, the canvas or a template, compiles a fresh plan and drops the cached ones.
- Updated dependencies [c44cca1]
- Updated dependencies [b506afc]
  - @lumiscia/lumen-preview@0.3.1

## 0.3.0

### Minor Changes

- a48252c: Shape, path, text and `transform` nodes take a `motion_blur` shutter angle in degrees, which smears the layer along its motion over a shutter centred on the frame. Expressions can now be evaluated between frames, so `frame` and `time` may be fractional during the shutter.
- 20ce332: `crop` now keeps its region in place and `resize` gains `x`/`y` placement, and both accept animated sizes instead of fixing them at compile time.
- 3b3147f: Shape, path and text nodes take their own `scale`, `rotation`, `anchor` and `opacity`, applied analytically so scaled vector edges stay sharp, and paths and polygons can have up to 16384 points instead of 128.
- 96edee6: Add `--bitrate`, `--crf`/`--qp` and `--preset` encoder quality flags to `lumen-local`, backed by new `rate_control` and `preset` fields on `VideoEncoderConfig`.
- 577d694: Add expression animation builtins: `progress`, `tween`, the `in`/`out`/`in_out` quad, cubic, expo and back easings, `cubic_bezier` and `spring`, plus `s` and `ms` suffixes on numbers. `vec2` values now support `+`, `-` and scaling, and `lerp`/`tween` work on numbers, `vec2` values and colours. Colours are expression values (`#rrggbb`, `#rrggbbaa`, `rgb()`, `rgba()`), so colour and paint params accept expressions. Compositions can declare reusable `functions`, such as `"rise(start)": "tween(vec2(0, 60), vec2(0, 0), start, 16, 'out_expo')"`, which expand where they're called. The builtin table, easing names and globals are published in the schema and as `expressionBuiltins`, `easingNames`/`EasingName` and `expressionGlobals` in the types.
- 00b258d: Expressions now see fractional `frame` and `time` values, and `time_remap` keeps fractional target frames instead of flooring them.
- 6360031: Replace the box blur in `blur` and `shadow` with a shared separable Gaussian that supports fractional and large radii.
- ef68eec: Text at large sizes now draws glyphs bigger than the atlas, renders MSDF with sharp corners at 1000 px and above by raising the field resolution with size, and adds an `auto` render mode that switches to MSDF from 512 px.
- 3f45d73: The GPU renderer now renders unchanging parts of a composition once and reuses them: a node whose params hold no expression and that reads no media or shader time, fed only by such nodes, is neither bound nor dispatched again on later frames of its plan. Output frames are unchanged. Kept output lives in the plan's own textures, so it counts against the plan cache budget and is dropped with its plan, and recompiles, font loads and texture replacements render it again.

  `memo` now caches. Its `cache_id` and `allow_expressions` params are removed in favour of `key`, an expression evaluated at the frame being rendered: while it gives the same value as on the frame `memo` last rendered, `memo` shows that output and nothing only it reads is bound or dispatched. `"key": "=floor(frame / 4)"` runs a heavy shader on one frame in four, and `"=floor(frame / 2)"` animates on twos. What `memo` holds is the source as rendered on the first frame of each key value that is rendered, so pair it with a `time_remap` to `floor(frame / 4) * 4` when frames may render out of order. The snapshot belongs to the memo node and its time-remap path rather than to a plan, so fractional frames and upstream switches that bind another plan show it too; it is taken again once every plan holding it is evicted, and any edit to the composition drops it, since a key holds output across frames, never across edits. Bounds a `crop` publishes are kept with the pixels. A literal or missing `key` adds nothing to the caching above. From TypeScript, `composition.addNode({ type: "memo", params: { key: floor(frame.div(4)) } })`.

  `GpuCompositionRenderer::cached_pass_count` and `submitted_pass_count` report how many passes were skipped and submitted, and `lumen-local` prints the per-frame averages as `static_cached` and `passes`.

- 137382c: Merge gains `mask_mode` (alpha or luma), `invert` and `feather` mask options, the boolean node keeps antialiased edges at its default threshold, and shapes can now be 0 pixels wide or tall to draw nothing.
- 076bf72: Text nodes take `glyph_offset`, `glyph_scale`, `glyph_rotation` and `glyph_opacity`, evaluated for every glyph to stagger glyphs, words and lines apart. Inside them, expressions can read the `glyph`, `word` and `line` index of the glyph being drawn and the `glyph_count`, `word_count` and `line_count` of the text, such as `"glyph_opacity": "=tween(0, 1, 10 + glyph * 2, 12)"`; elsewhere those globals are an error. Each glyph scales and turns about the middle of its advance and line, before the text's own transform, and fades before its `opacity`. `@lumiscia/lumen-shared` exports them as `glyph`, `word`, `line`, `glyphCount`, `wordCount` and `lineCount`.
- f6e2ddb: Add `letter_spacing` and `line_height` to the text node and let `font_weight` take any value from 1 to 1000 instead of snapping to hundreds.
- 1b55b88: On Linux, `lumen-local --encoder av1_vulkan` (or `h264_vulkan`/`hevc_vulkan`) now renders and encodes on one shared Vulkan device, converting frames to NV12 on the GPU instead of reading them back. It falls back to the CPU upload path when the device has no Vulkan Video encode support or separate compute queue family, or when the frame size is odd. The new `VulkanDevice`, `VulkanRenderQueue` and `VideoEncoderConfig::vulkan_device` back `EncodeMode::GpuTexture(GpuBackend::Vulkan)`.
- 4e61e7a: `wgsl_shader` now takes up to 16 inputs (`input0..input15` plus a shared `input_sampler`), gives every shader `lumen.time`, `lumen.frame` and `lumen.resolution`, binds typed uniforms by name from the shader's uniform struct, rebuilds its pipeline when the source changes, premultiplies straight-alpha inputs before the shader sees them, and rejects unknown or malformed uniform values and shader bindings above 2 instead of ignoring them.

  Uniforms are set with a `uniforms` object instead of the `bindings` string, which is removed. Each value is a number, an array of 2-4 numbers or `=expression`s, a whole-value `=expression`, or, for `vec3<f32>` and `vec4<f32>` fields, a `#rrggbb` / `#rrggbbaa` colour or a colour expression such as `=rgb(255, 128, 0)`. `wgslShader()` values also take typed `Expr`s from the expression helpers. The schema and the generated `ShaderUniforms` / `ShaderUniformValue` types describe it, and `wgslShader()` in `@lumiscia/lumen-shared` declares the uniform struct in TypeScript and type-checks each field's value.

  The GPU renderer now keeps compiled plans within an estimated memory budget (1 GiB by default, configurable with `GpuCompositionRenderer::set_plan_cache_budget`), dropping least recently used plans instead of keeping one per animated switch or shader combination.

### Patch Changes

- 36c4af9: Tag encoded video as BT.709 with matching color conversion, encode fractional frame rates such as 29.97 exactly, and rebuild the RGBA converter when decoded frame geometry changes.
- 86eda9e: Shapes, paths, solid-colour text and transformed layers render into textures sized to the pixels they cover rather than the whole canvas, and merges, multimerges, opacity and colour filters work within those bounds, cutting GPU memory and frame time for compositions with many small elements. Output frames are unchanged.
- 2cf81aa: Parse and render errors now name the node and property involved, and colour expressions are rejected when the composition is parsed instead of when it renders.
- cf195bd: Fractional values for integer params now report "must be a whole number", and compositions still using the removed `bindings` param on `wgsl_shader` are pointed to `uniforms`.
- d52b7d6: Allow `=` expressions in the schema and TypeScript types exactly where the engine accepts them. Paints, colours, named enums and `alpha_premultiply.mode` no longer type-check with an expression.
- 15a1db6: Reject lossy audio volume and integer values with errors that name the field, report which part of an invalid color was wrong, and drop the unused `x-lumen-nodeKinds` schema key. Native GPU instances now discard HAL debug labels by default to avoid a Vulkan loader crash with concurrent devices; set `WGPU_DISCARD_HAL_LABELS=0` to keep resource names in GPU captures.
- 17d63ec: Allow string params to start with a literal `=` by escaping it as `==`: `"==wow"` renders `=wow`, while `"=frame"` is still an expression.
- e40a08b: Reject invalid timeline fps, missing or invalid duration frames, malformed background colors, out-of-range color components, malformed gradient stops and unparsable WGSL shader bindings with clear errors instead of silently falling back to defaults.
- 0a08ad1: Render inputs connected from node 0 instead of treating them as unconnected, which left the first node of every SDK-built composition transparent.
- d8e548e: Sample Transform and Resize at pixel centres so identity transforms with linear sampling reproduce the input exactly, and add a Transform `custom_pivot` option so a (0, 0) pivot can mean the top-left corner instead of the source centre.
- d39faea: Render `shape` polygons from `polygon_points` instead of drawing a rectangle, and draw nothing while the point list is empty or incomplete.
- 6bc1c81: Treat rasters as premultiplied alpha consistently. Merges no longer darken translucent overlays, shape and path strokes composite over fills correctly, gradients interpolate in premultiplied space, and colour adjustments keep fully transparent pixels transparent. Compositing nodes and the default WGSL shader now honour straight-alpha inputs, and the Alpha Premultiply mode must be a constant rather than an expression.
- 341b369: Accept a single number for a gradient `radius`, as the schema allows, and name the failing field when a paint fails to parse instead of a generic "expected paint" error.
- 2b50b73: Add opt-in `render_mode: "msdf"` text rendering through the `msdf` Cargo feature (enabled by default). Raster and MSDF glyphs share a fixed 4096² RGBA8 atlas costing 64 MiB per renderer once text is used. Standalone GPU callers can install the atlas with `CompiledComposition::prepare_text_atlas`.
- 3712329: Draw raster text on whole pixels so it stays sharp at fractional positions, keep glyph memory bounded when font sizes animate, warn instead of silently dropping glyphs when the atlas is full, measure relative text gradients against the text's own bounds, render colour emoji without dark fringes, and make `textWidth` use the same fonts as text rendering.
- e030363: Keep preview commands applied while a render is in flight, reuse the GPU device across composition and media changes, and bound the wasm video frame cache by bytes through `frameCacheCapacity`.
- b0bbf52: Default new compositions to 30 fps in the TypeScript builder, matching the engine, and convert audio `start_frame`/`duration_frames` at the composition's frame rate instead of a fixed 30. Compositions that relied on the old 24 fps builder default now play faster (240 frames last 8 s instead of 10 s); call `setFps(24)` to keep the old timing.
- 0aaa4a1: Clamp negative shape border radii to zero so expressions cannot distort rectangles.
- 9c0b0b7: Bind each time-remapped branch with its own frame parameters when a node is reached through more than one time context.
- fcb8095: Update the Rust engine dependencies, including wgpu 30 and cosmic-text 0.19.
- Updated dependencies [482ec57]
- Updated dependencies [e030363]
  - @lumiscia/lumen-preview@0.3.0

## 0.2.2

### Patch Changes

- 9d1fc11: Fix gradient paint rendering for conic angle offsets and text fills.
  - @lumiscia/lumen-preview@0.2.2

## 0.2.0

### Minor Changes

- 072169a: Replace legacy node property metadata with typed node params.
- cb6f4dc: Add external GPU frame targets and GPU media import plumbing for lower-copy render and encode paths.

### Patch Changes

- Updated dependencies [072169a]
- Updated dependencies [cb6f4dc]
  - @lumiscia/lumen-preview@0.2.0

## 0.1.0

### Minor Changes

- 906481a: Prepare the Lumen TypeScript package family for coordinated public releases.
