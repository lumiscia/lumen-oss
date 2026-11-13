# @lumiscia/lumen-types

## 0.6.0-beta.2

No changes in this release.

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

- 89568da: A node param is declared as a media source in one place, and a composition can list what it needs before it renders.

  A param struct marks a `String` param with `#[media(image, video)]`, `#[media(model)]` or `#[media(environment)]`, and `by = kind` names the enum param that picks among several kinds. `media_in.source`, `model_3d.source`, `render_3d.environment_map` and an audio clip's `source_id` are declared this way. Asset validation, the schema and the browser preview now read the declaration instead of each knowing which nodes take sources.

  `Composition::media_dependencies()` returns the distinct sources a composition's nodes and audio clips name as written, with the kinds each use reads and where it is used (node and param, or audio clip, inside a template too). A source an expression computes is listed as dynamic, and the font families text uses are listed apart from media. The browser bindings expose it as `mediaDependencies(compositionJson)`, typed as `MediaInventory` in `@lumiscia/lumen-types`.

  The generated schema gives each such param `x-lumen-media` (`{ "kinds": ["image", "video"], "selectedBy": "kind" }`), and `@lumiscia/lumen-types` exports the generated `nodeMediaSources` table (node kind → param → kinds) with `MediaSourceParam` and `MediaSourceKind`. The preview reads it to tell what a `media` entry holds.

  **Breaking for Rust:** `PropertyDef` has a `media` field, and the `Node` and `NodeParams` traits have a required `for_each_media_source`; hand-written implementations must add it (the `Node` and `Delegate` derives do). `lumen_engine::media::resolve_asset_of` accepts any of several kinds. `LumenPreviewBindings` has a required `mediaDependencies`, so a bindings object built by hand must add it. Compositions change in one way only: a name in `compositions` may be at most 256 bytes long; a longer one fails with `document.invalid`.

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

- ac4bc51: Compositions can be built as nested node values. `nodes` has one function per node kind, named after its `type`, that takes the node's params and input ports by name in one object and returns the `NodeInput` `addNode` takes, without adding anything. A port takes a node value as well as a node, and `composition.addOutput(source)` adds a `media_output` reading it. A value is added when a node reading it is, after its own inputs, so it gets the ids and JSON the same `addNode` calls would; the same object read twice is one node with two readers, and a value mutated into a cycle is an error. Ports keep their kinds: a scene value in a raster port, or a raster in `render_3d`'s `scene`, doesn't compile.

  ```ts
  import { Composition, nodes as n } from "@lumiscia/lumen-sdk";

  const scene = n.merge_3d({
    objects: [n.camera_3d({ position: [0, 1, 6] }), n.shape_3d({ shape: "torus" })],
  });
  const composition = new Composition().addOutput(
    n.raster_multimerge({
      layers: [
        n.render_3d({ scene, tone_mapping: "agx" }),
        n.glow({ glow_only: true, radius: 46, source: n.render_3d({ scene, exposure: -1 }) }),
      ],
    }),
  );
  ```

  `defineComposition`'s `build` may return a node value, and a `clip()` nests in one like any other node. `addNode` takes node values in its `inputs` too, and values may read nodes `addNode` returned.

  `@lumiscia/lumen-types` adds `nodeInputPorts`, each kind's input ports with their kind and whether they take a list, and `NodeParamPortNameClash`, which is `never`: the generated types stop compiling if a kind ever gives a param and an input port the same name.

  `addNode` itself still adds a node every time it is called. A failed `addNode` leaves the composition as it was, nested values included.

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

- 49b2878: Node inputs and outputs have a kind, `raster` or `scene`, so a scene can't be wired where a raster goes or the reverse. A connection joining a node's output to an input of another kind fails validation with `connection.wrong_kind`, naming both nodes, the input and the two kinds, and its diagnostic points at the connection. The generated definitions give each port's and output's kind as `raster` or `scene`, where they said `raster_frame`, and list the kinds as `x-lumen-portKinds`; `NodePortKind` is `"raster" | "scene"`, and `NodeOutputKind` and `NodeInputPortKind` give a node kind's output and an input's kind. In the SDK, `addNode` inputs and `connect` only take nodes whose `type` outputs what the port takes. `NodeReference<"scene">` or `NodeReference<"raster">` is a node for a port of that kind, and a plain `NodeReference` is any node; a bare id isn't checked.
- 3ab8aa0: Params can now hold three numbers, written `[x, y, z]` in JSON or as an expression, ready for the 3D nodes' `position`, `rotation` and `scale`. Expressions gain `vec3(x, y, z)`, and vec3 values work like vec2 values: they add and subtract with each other, multiply and divide componentwise or by a number on either side, negate, and interpolate in `lerp`, `tween` and `keyframes`, e.g. `"=tween(vec3(0, 0, -400), vec3(0, 0, 0), 0, 24, 'out_expo') + vec3(0, 10 * sin(time), 0)"`. Mixing a vec3 with a vec2 is an error. Template props can be declared `"type": "vec3"`, and a `wgsl_shader` vec3 uniform takes a vec3 expression. In `@lumiscia/lumen-shared` the helper is `vec3`, which returns an `Expr<Vec3Value>` that TypeScript keeps apart from colour expressions, so a vec3 can't reach a colour param or share a `lerp` or keyframe track with a colour. `prop.vec3(..)` declares a prop, and params and prop defaults take a plain `Vec3` tuple `[x, y, z]`. A three-number tuple passed to a helper still means `rgb(..)`, so write `vec3(x, y, z)` there. `vec3` is now a reserved name, so composition functions and their parameters can't use it.

### Patch Changes

- d09b553: Documentation for 3D. The `lumen-sdk`, `lumen-shared` and `lumen-preview` READMEs link to the new 3D guide, and `lumen-shared`'s shows a scene. The descriptions of a 3D material's `color` and `shadow_catcher` params, which `lumen-types` carries as doc comments and `lumen-bindings` as node metadata, now say that only an image plane has a raster to multiply by `color` or to count in `shadow_catcher`. Nothing about how a composition renders changes.

## 0.5.1

### Patch Changes

- 6285ab0: Expressions gain the `sine`, `quart`, `quint`, `circ`, `elastic` and `bounce` easing families (`in_`, `out_` and `in_out_` each, e.g. `out_bounce(t)` or `tween(0, 1, 0, 30, 'in_out_elastic')`), and the functions `sign`, `atan2`, `hypot` and `remap`. `@lumiscia/lumen-shared` exports a helper for each.
- 87164d8: Render errors name the node by its JSON `type`, like `media_in node 3 evaluation failed`, instead of a Rust-style `MediaIn`. `motion_blur` now says in its description that it blurs content that already has motion blur a second time, and `lumen-local --help` no longer reads as if a quality preset targeted 14 Mb/s.

## 0.5.0

### Minor Changes

- 6cddffa: A `motion_blur` node blurs any motion in its source, including what the analytic `motion_blur` shutter of shapes, paths, text and `transform` can't see: animated `width`, `height`, `font_size` and `border_radius`, path data, anything upstream of a `transform`, and motion inside video. `{ "type": "motion_blur", "params": { "samples": 8, "shutter": 180 } }` renders its source at `samples` sub-frame times across a `shutter` of that many degrees, centred on the frame like the analytic shutter, and averages them in premultiplied space. The result is the average of the sub-frames' 8-bit renders, rounded to the nearest level. Each sample renders the whole source again, so the cost grows with `samples`; the samples share the same pooled textures, so memory doesn't. Video shows the source frame each sub-frame time maps to, and media prediction asks for those frames. `samples` is 1 to 64 and `shutter` 0 to 360; other values fail the render. `samples: 1` or `shutter: 0` renders the source once, unchanged. While a `motion_blur` samples, analytic motion blur inside it is off, and a `motion_blur` inside it renders its source once per sample.
- db420ff: Source fields accept asset references, `asset:<id>`, naming a file in the host's asset store: `media_in`'s `source`, an audio clip's `source_id`, and a text node's or span's `font_family`. The `asset:` prefix is a scheme, so it can't be mistaken for a relative path, and plain paths and the `media` manifest keep working as before. An id the host doesn't have is an `asset.not_found` error, and an asset of another kind than its field reads, such as a font used as an image, is `asset.wrong_kind`. A host gives the engine its store as an `AssetResolver`, which resolves an id to an `Asset` (an image, video or audio file, or a font family's face files), and passing it to `json::check` checks literal references before rendering. `lumen-local` has no asset store, so references are `asset.not_found` errors there, and `lumen-server` resolves them against its media uploads.

  A font asset is one family, and weight and style choose among its faces. Fonts now shape under the name the composition asks for: a font staged under a `media` alias, or any file whose faces carry another family name, used to load and then silently fall back, and now draws as asked. Its faces answer to that name instead of their own, so they can't stand in for another font with the same internal family name. For Rust users, `MediaStore::get_font_resolver` returns a `Result`, and `json::check` and `check_value` take the asset store as a third argument.

- d21228a: `path` strokes take a dash pattern. `stroke_dash` lists up to 16 non-negative lengths in untransformed pixels that the stroke alternately draws and skips, starting with a dash; an odd number of lengths repeats, as in SVG. Each length takes a literal or an expression. Every dash has round caps, so a dash of length 0 draws a dot, and dashes that cross a corner join as a solid stroke does. `stroke_dash_offset` shifts the pattern along each contour and takes an expression, so `"=frame"` marches the dashes. Dashes are measured along the whole contour, and `trim_start` and `trim_end` then clip them. An empty or all-zero pattern strokes solidly. Undashed strokes no longer distort segments shorter than 0.01 units, and an untrimmed stroke now draws the whole of a segment that is very short next to its contour, where it was drawn only halfway.
- 4df3509: Diagnostics reach the TypeScript packages. The wasm bindings add `check(json, fonts?)`, which returns what is wrong with a composition without loading it, and a composition `loadComposition` rejects throws with the same `diagnostics`. `LumenError` in the SDK has `diagnostics`, filled when a render is rejected for an invalid composition. The preview and both `Player` components take `onError`, which receives each failure with its diagnostics. `LumenDiagnostic` is exported from `@lumiscia/lumen-types`.
- 428e0a7: `directional_blur`, `radial_blur` and `zoom_blur` nodes average their source along a path through each pixel, centred on it. `directional_blur` spreads each pixel along a line `distance` pixels long at `angle` degrees clockwise from horizontal. `radial_blur` spins it along an arc of `amount` degrees, up to 360, about `center`, and `zoom_blur` streaks it along the ray from `center` over `amount` times its distance from it. `edge` is `clamp`, which repeats the canvas's edge pixels, or `transparent`. `zoom_blur` samples its streak in equal ratios of scale, so it reaches farther away from `center` than towards it. Each blur takes about one sample per pixel of its longest streak, up to 4096, so streaks up to 4096 pixels long stay smooth; streaks longer than about 256 pixels are taken in two passes whose second resamples the first, which softens sharp edges by a few levels more than single samples would, and streaks from 64 to 256 pixels crossfade between the two so that animating a blur never jumps. The output is sized to the source grown along the path it is blurred over.
- 8a28aa1: A `displacement_map` node moves each pixel of its `source` input by the pixel of its `map` input over it, in a single pass. `channel_x` and `channel_y` pick the map channel that drives each axis, `red`, `green`, `blue` or `luma`, and `amount_x` and `amount_y` are the most pixels it moves: a byte of 0 moves the source the whole amount left or up, 255 nearly as far right or down, and 128 leaves it in place. Map bytes are premultiplied by alpha, so transparent areas of the map, and everything outside it, displace nothing, and a neutral grey stays neutral as it fades. `edge` is `clamp`, which repeats the source's edge pixels, or `transparent`, and `sampling` is `linear` or `nearest`. The output is sized to the source grown by the amounts where the map overlaps it.
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
- 1d76f93: `path` fills take an `offset` and a `feather`, both in untransformed pixels and both animatable. A positive `offset` grows the fill outward, rounding the corners it grows around, and a negative one shrinks it; a fill shrunk past its middle vanishes, and the stroke stays on the original outline. `feather` fades the fill's edge over that width, centred on the edge, without a blur pass. Under non-uniform scale both stretch with the path, as `stroke_width` does. At 0, the default, fills render as before, and an offset or feather animated to 0 meets them there. Edges that coincide and cancel under the even-odd rule, such as the side two adjoining squares share, are no longer edges of the fill: they are neither offset nor feathered, and sharp fills no longer show a faint seam along them.
- 404f117: Renders pick their quality by preset and their codec by name. Every host defines `draft`, `standard`, `high` and `lossless`, each with settings per FFmpeg encoder, and `render_settings` in a composition gains `codec` (`h264`, `hevc` or `av1`) and `quality` (a preset name), which the `RenderSettings` type and `new Composition({ renderSettings: { codec, quality } })` carry. Templates' render settings are now `TemplateRenderSettings`, without them. Previews ignore both.

  A render with no preset now uses `standard` instead of a 14 Mb/s target. Each codec tries its encoders in order, with one warning for each that won't open: on Linux the Vulkan encoder, then `libx264`, `libx265` or `libsvtav1`, and on macOS VideoToolbox, then the same software encoders. A preset only tries the encoders it has settings for, so `lossless` uses `libx264`, `libx265` or NVENC H.264 and HEVC. `LUMEN_VIDEO_ENCODER` still pins one encoder with no fallback. On `lumen-server`, a preset, codec or pinned encoder this FFmpeg build can't serve is a `422` when the render is requested. An output container that can't hold the codec, such as H.264 in `.webm`, fails when the encoder is created.

  `LUMEN_QUALITY_CONFIG` (and `lumen-server --quality-config`) names a JSON file that sets the default preset and codec, each codec's encoder order, and presets' settings per encoder: `crf`, `qp` or `bitrate`, an encoder `preset`, and raw FFmpeg `options`. It is checked when `lumen-server` starts or `lumen-local` loads it, so an unknown field, codec, encoder or option, or a value outside an option's range, fails then instead of on a render.

  `lumen-local` gains `--quality`, `--codec` and `--quality-config`. `--bitrate`, `--crf`, `--qp` and `--preset` keep their meaning, replacing the preset on every encoder tried, and can't be combined with `--quality`. `--crf`, `--qp` and `--preset` now also work on `libx265` and `libsvtav1`.

  For Rust users of `lumen-engine-ffmpeg`, `QualityConfig` loads and resolves presets into a `VideoEncoderChoice`, which holds the ordered `EncoderCandidate`s a render tries; `EncoderQuality` gains raw `options`, and its `rate_control` is optional, so `EncoderQuality::default()` and `VideoEncoderConfig::cpu_rgba` leave rate control at the encoder's default. `VideoEncoderChoice::resolve` and `check_quality` are gone. In `lumen-server`, `RenderOptions::encoder` and `resolve_encoder` replace `video_encoder` and `quality`, and `Mp4Render::open` takes the choice.

- ef422d1: Radial gradients take an optional `focal` point, which makes them two-point conical gradients for off-centre highlights. The first stop sits at the focal point and the last on the circle at `center` and `radius`. A focal point on or outside that circle is moved just inside it. Without `focal`, or with `focal` equal to `center`, gradients render as before.
- 5db9958: Merge and RasterMultiMerge gain the add, difference, exclusion, color dodge, color burn, hard light and soft light blend modes.

### Patch Changes

- 92b28e3: Every package now has a README, so its npm page describes it.

## 0.4.0

### Minor Changes

- c1c57b2: `addNode` takes `inputs`, so a node connects to the nodes that feed it where it is added: `addNode({ type: "merge", inputs: { base: background, overlay: title } })`. A variadic port such as `layers` takes an array, `{ layers: [background, image, title] }`. Ports are typed from the node kind, and the new generated `NodeVariadicInputPortName<Kind>` marks the ones that take arrays. The connections are the ordinary ones, in the order written, so the JSON is the same as with `connect`, which stays. `literal(text)` escapes a leading `=` as `==`, and `expr("if(frame > 30, 1, 0)")` wraps raw expression text, such as `if(..)` and `node(..)`, as an `Expr`.

  Breaking: `Composition` loses `setSize`, `setFps`, `setDurationFrames`, `setDurationSeconds`, `setMetadata` and `setAudio`; pass `renderSettings`, `timeline` and `metadata` to the constructor. `connect` loses `fromPort`, which the engine only accepts as `output`. `connect` and `inputs` throw for a node handle from another composition; numeric ids are left to the engine. A failed `addNode` leaves the composition, including its templates, functions and next automatic id, as it was. A composition definition no longer has `.composition`. `moves(base, [{ by, .. }])` emits `by * tween(0, 1, ..)` instead of `tween(0, by, ..)`, with the same value, and `Expr` no longer tracks a kind. `fn` and `addFunction` no longer check names and `wgslShader` no longer checks uniform names or types; the engine rejects function names when the composition loads, and the shader compiler reports bad uniform names and types. A `toJSON` on a plain array in a param, or a non-enumerable one on a plain param object, is no longer called: `{ text: "draft" }` with a hidden `toJSON` returning `{ text: "final" }` now serializes as `draft`. An enumerable `toJSON` on a plain object still runs, and so does one on a class instance or array subclass.

- b6ece6b: Composition errors now name the JSON path, such as `timeline.fps: ...`, and more mistakes fail instead of being ignored. Unknown keys in `timeline` and `render_settings` are errors, including in templates, so a typo like `fpss` no longer falls back to 30 fps. Other unknown keys at the root and in `audio` are still ignored.

  `render_settings.background_color`, which rendering never read, is an error: connect a `background` node instead. `Composition.setBackgroundColor` is gone. Audio clips need whole-number `start_ms` and `duration_ms`, and `source_start_ms` must be a whole number when set; `start_frame`, `duration_frames` and `source_start_seconds` are errors. `schemaVersion` is ignored and the schema no longer types it, `lumenSchemaVersion` must be absent or `0.1.0` in the schema too, and a non-string `$schema` or a `metadata` that isn't an object with a string `name` now fails at load, as it already did in the schema. The builder no longer writes `durationSeconds` into `timeline`. `CompositionBuilder::build` now rejects fps that isn't positive and finite, a zero size or duration, and a prop default of the wrong kind, for the root and templates.

- 6b3a821: `Composition.connect()` takes the target port explicitly instead of defaulting to `input`, which no node has, so a connection without `toPort` no longer builds a composition the engine rejects. Given the target node, `toPort` is typed to its inputs, from the new generated `NodeInputPortName<Kind>` type: `composition.connect(background, merge, { toPort: "base" })` type-checks, and `toPort: "source"` on a `merge` does not. `addNode` now infers the node kind it returns.
- 0003560: Expressions fail earlier and accept more number forms. An unknown variable such as `=frmae + 1` is now rejected when the composition loads instead of failing mid-render, an unknown string escape such as `'\q'` is an error rather than dropping the backslash, and `node(id)` rejects ids above 2^53. Numbers may now use exponents and a leading dot (`1e3`, `1.5e-3`, `.5`), and a number too large to represent is rejected rather than becoming infinity. The SDK's helpers write large and small numbers in exponent form as a result.

  The `linear(a, b, t)` builtin and the SDK's `linear` helper are gone; use `lerp`. `'linear'` stays as an easing name. `x-lumen-expressions` in the schema now lists `forms` (`if` and `node`) and `reserved` names, which `@lumiscia/lumen-types` exports as `expressionForms` and `expressionReservedNames`.

- 75572c3: The composition schema's paint, shader uniform and clip prop `$defs` are now generated from the Rust param definitions, with the same accepted values as before. New `$defs` name the shapes that were inline: `gradientPaint`, `gradientStop`, `paintKind`, `gradientUnits`, `gradientSpread`, `gradientInterpolation`, `shaderUniformComponent`, `shaderUniforms` and `clipProps`; gradient fields carry descriptions, defaults and enum options. `@lumiscia/lumen-types` generates `Paint`, `GradientPaint`, `GradientStop`, `ShaderUniforms`, `ShaderUniformValue`, `ShaderUniformComponent`, `ClipProps` and `ClipPropValue` from these `$defs`, with JSDoc, and exports the gradient enums as `PaintKind`, `GradientUnits`, `GradientSpread` and `GradientInterpolation`.

  For Rust: `GradientPaintDelegate` and `GradientStopDelegate` are exported from `vector::paint`, and the gradient enums implement `NodeEnum` with an `enum` property kind.

  Required fields and enum aliases come from the Rust types: `#[meta(required)]` on a `Delegate` field marks it required in the schema and, for `#[delegate(properties)]` structs, makes deserializing reject objects without it. `PropertyDef` gains `required` and `EnumOptionDef` gains `aliases`, read from the variant's `#[serde(alias)]` attributes.

- eb354e6: The text node draws underlines, strikethroughs and highlights. The new node-level `underline`, `strikethrough` and `highlight` params are off when absent and apply to string content and to every span that leaves its own unset. A span's own decoration replaces the node's, and `null` turns it off for that span.

  Consecutive spans that take the node's decoration form one run, which draws one line or box per line of text, so mixed font sizes don't step or seam. A line's absent `thickness` and `offset` come from the tallest span on that line, and its absent `paint` is the text's own paint. Highlights break per line like CSS `box-decoration-break: clone`: each line gets its own padding and corner radius. `extent` maps `0..1` along the run's text in reading order, continuing across line breaks and following right-to-left text, so `"extent": "=vec2(0, clamp(time * 2, 0, 1))"` draws an underline in across wrapped lines. Lines can be `solid`, `double`, `dotted`, `dashed` or `wavy`.

  Decorations are painted in CSS order (highlights, underlines, glyphs, then strikethroughs). They follow the node's transform, opacity and motion blur but not the `glyph_*` params. They never change the text's layout or `text_width` and `text_height`, and they count toward the text node's glyph limit. Text without decorations renders as before.

  For Rust: `TextParams` has `underline`, `strikethrough` and `highlight` fields. `GpuGlyphInstance` has a `GLYPH_MODE_RECT` render mode for decoration rects, made with `GpuGlyphInstance::rect`, and `ShapedText::decorate` lays out a `TextDecoration`. Node params can now be `Option<T>`, unset unless given, and their specs then have no default value. The generated `NodeParamSpec.defaultValue` is optional.

- 4f3894f: The text node takes a `font_features` param, and spans take the same field: OpenType features by four-character tag, such as `{ "tnum": 1, "liga": 0, "ss01": 1 }`. A value of `0` turns a feature off, `1` turns it on and larger values select an alternate. A span's features merge over the node's: the span's keys win and the rest are inherited. Tags must be exactly 4 printable ASCII characters and values non-negative integers, and errors name the field. Each value takes a literal or an `=expression`, but the whole map cannot be an expression.

  Features shape the text, so `text_width`, `text_height` and `text_bounds` follow them. A word takes the features of the first span that starts it, because cosmic-text shapes the spans of a word together. Text without features renders as before.

  For Rust: `lumen_text::TextSpan` has a `font_features` field of `FontFeature` settings, and the new `FontFeatures` param kind is generated as `FontFeatures` in `@lumiscia/lumen-types`.

- 93fb454: The text node can limit its text to a number of lines or a box, and shrink text to fit. The new `max_lines` and `max_height` params drop the lines past them, and are unlimited when absent. `overflow` sets how the last line shown ends: `clip`, the default, leaves it as it is, and `ellipsis` ends it with `…`. Clusters at that line's end, and whitespace before the ellipsis, are dropped until it fits `max_width`. The ellipsis goes at the line's end in reading order, so it sits on the left of right-to-left text, and it takes the style, paint and decorations of the text it follows.

  `fit: "shrink"` scales every font size, the node's and each span's, by one factor of at most 1, so the text fits `max_width`, `max_lines` and `max_height` without breaking a word. Letter spacing, a pixel `line_height`, and decoration thickness, offset, padding and radius scale with it. The factor is the largest that fits, down to the one that takes `font_size` to `min_font_size` (8 by default); lines that still don't fit there are dropped by `overflow`. A fit takes at most 12 shapes, and is kept for the text's layout, so measuring, bounding and drawing the same text share one fit, and constant text pays for it once.

  `text_width`, `text_height`, bounded output and decorations all cover the text as drawn, after truncation and fit. The `glyph_*` params count only the glyphs and lines shown. Glyphs keep their numbers, and the ellipsis is one glyph numbered after the last. Text that sets none of these params lays out as before.

  For Rust: `TextParams` has `max_lines`, `max_height`, `overflow`, `fit` and `min_font_size` fields. `TextLayoutRequest` has `max_lines`, `max_height`, `overflow` and `min_fit_scale`, and `ShapedText::fit_scale` gives the factor a fit chose.

- 5a0939c: Add the `text_content`, `text_line` and `text_highlight` param kinds for rich text. No node uses them yet. Text content is a string or an array of spans. Each span needs `text` and can override the node's font family, size, weight, style, letter spacing and color, and can set an underline, strikethrough or highlight; `null` turns off a decoration the span would otherwise inherit. Span fields, including enums, accept `=expressions` like node params, and evaluation errors name their path, such as `content[1].font_weight`. The composition schema describes these kinds in `$defs` (`textContent`, `textSpan`, `textLine`, `textHighlight`, `textLineStyle`), which are generated from the Rust param definitions. `@lumiscia/lumen-types` exports matching `TextContent`, `TextSpan`, `TextLine`, `TextHighlight` and `TextLineStyle` types.

  For Rust: nested `Delegate` structs can now have `Option<T>` fields, which are left out when absent, and `Override<T>` fields, which inherit, clear or set a value. A struct delegate marked `#[delegate(properties)]` parses its nested JSON field by field the way node params do, rejects unknown fields, and has a `to_json` method; other struct delegates still deserialize through serde. `PropertyDef` has a new `nullable` field. `TextFontStyle` accepts expressions, which also lets the text node's `font_style` take one.

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

- e3d02e6: `boolean` `operation`, `raster_multimerge` `blend_mode` and `color_grade` `interpolation` are now real enums, so a value an enum doesn't have is an error instead of falling back to the first option. `color_grade` `interpolation` takes `"nearest"` or `"linear"` instead of `0` or `1`, and `shadow` `color` fails with a clear message when it is a gradient, which it used to replace with the gradient's first stop. `merge` `blend_mode` and the `sampling` of `resize` and `transform` now also accept expressions.
- ca87eaa: `@lumiscia/lumen-types` now generates every node and composition type from `composition.schema.json`, so node params carry their schema descriptions as JSDoc and the root types can no longer drift from the schema. Enums that several nodes share are one type (`BlendMode`, `LoopMode`, `MaskMode`, `ChannelSelector`, `MediaInSourceKind`) and the old per-node names are aliases of it. New exports are `AudioClip`, `AudioTrack`, `RenderSettings` and `Timeline`. `MetaManifest` is gone, and `Connection`, `CompositionTemplate` and `LumenComposition` are type aliases rather than interfaces.

  The composition schema defines `timeline` and `render_settings` once in `$defs` and gives `metadata`, `$schema` and `lumenSchemaVersion` root types: a non-string version or `$schema`, or a `metadata` that is not an object with a string `name`, is now rejected. The `fps` default of 30 moves from the schema to its description. Consumers that vendor the schema need the new copy.

## 0.3.1

No changes in this release.

## 0.3.0

### Minor Changes

- 2c68ade: Add an animatable opacity processing node with correct premultiplied and straight-alpha handling.
- 5726783: Render SVG path commands with adaptive curve flattening, including relative, smooth, quadratic, and closed subpaths.
- 577d694: Add expression animation builtins: `progress`, `tween`, the `in`/`out`/`in_out` quad, cubic, expo and back easings, `cubic_bezier` and `spring`, plus `s` and `ms` suffixes on numbers. `vec2` values now support `+`, `-` and scaling, and `lerp`/`tween` work on numbers, `vec2` values and colours. Colours are expression values (`#rrggbb`, `#rrggbbaa`, `rgb()`, `rgba()`), so colour and paint params accept expressions. Compositions can declare reusable `functions`, such as `"rise(start)": "tween(vec2(0, 60), vec2(0, 0), start, 16, 'out_expo')"`, which expand where they're called. The builtin table, easing names and globals are published in the schema and as `expressionBuiltins`, `easingNames`/`EasingName` and `expressionGlobals` in the types.
- 3f45d73: The GPU renderer now renders unchanging parts of a composition once and reuses them: a node whose params hold no expression and that reads no media or shader time, fed only by such nodes, is neither bound nor dispatched again on later frames of its plan. Output frames are unchanged. Kept output lives in the plan's own textures, so it counts against the plan cache budget and is dropped with its plan, and recompiles, font loads and texture replacements render it again.

  `memo` now caches. Its `cache_id` and `allow_expressions` params are removed in favour of `key`, an expression evaluated at the frame being rendered: while it gives the same value as on the frame `memo` last rendered, `memo` shows that output and nothing only it reads is bound or dispatched. `"key": "=floor(frame / 4)"` runs a heavy shader on one frame in four, and `"=floor(frame / 2)"` animates on twos. What `memo` holds is the source as rendered on the first frame of each key value that is rendered, so pair it with a `time_remap` to `floor(frame / 4) * 4` when frames may render out of order. The snapshot belongs to the memo node and its time-remap path rather than to a plan, so fractional frames and upstream switches that bind another plan show it too; it is taken again once every plan holding it is evicted, and any edit to the composition drops it, since a key holds output across frames, never across edits. Bounds a `crop` publishes are kept with the pixels. A literal or missing `key` adds nothing to the caching above. From TypeScript, `composition.addNode({ type: "memo", params: { key: floor(frame.div(4)) } })`.

  `GpuCompositionRenderer::cached_pass_count` and `submitted_pass_count` report how many passes were skipped and submitted, and `lumen-local` prints the per-frame averages as `static_cached` and `passes`.

- 076bf72: Text nodes take `glyph_offset`, `glyph_scale`, `glyph_rotation` and `glyph_opacity`, evaluated for every glyph to stagger glyphs, words and lines apart. Inside them, expressions can read the `glyph`, `word` and `line` index of the glyph being drawn and the `glyph_count`, `word_count` and `line_count` of the text, such as `"glyph_opacity": "=tween(0, 1, 10 + glyph * 2, 12)"`; elsewhere those globals are an error. Each glyph scales and turns about the middle of its advance and line, before the text's own transform, and fades before its `opacity`. `@lumiscia/lumen-shared` exports them as `glyph`, `word`, `line`, `glyphCount`, `wordCount` and `lineCount`.
- 2b50b73: Add opt-in `render_mode: "msdf"` text rendering through the `msdf` Cargo feature (enabled by default). Raster and MSDF glyphs share a fixed 4096² RGBA8 atlas costing 64 MiB per renderer once text is used. Standalone GPU callers can install the atlas with `CompiledComposition::prepare_text_atlas`.
- 97f55bf: Harden runtime rendering and preview lifecycle cleanup, add configurable local font discovery, and make CUDA frame ownership explicit in the Rust API.
- 1b30cb7: Add animatable start and end trimming for path strokes while keeping fill geometry intact.
- b88aeda: Add typed `vec2` expression values for animating vector node parameters and referencing vector properties.
- 4e61e7a: `wgsl_shader` now takes up to 16 inputs (`input0..input15` plus a shared `input_sampler`), gives every shader `lumen.time`, `lumen.frame` and `lumen.resolution`, binds typed uniforms by name from the shader's uniform struct, rebuilds its pipeline when the source changes, premultiplies straight-alpha inputs before the shader sees them, and rejects unknown or malformed uniform values and shader bindings above 2 instead of ignoring them.

  Uniforms are set with a `uniforms` object instead of the `bindings` string, which is removed. Each value is a number, an array of 2-4 numbers or `=expression`s, a whole-value `=expression`, or, for `vec3<f32>` and `vec4<f32>` fields, a `#rrggbb` / `#rrggbbaa` colour or a colour expression such as `=rgb(255, 128, 0)`. `wgslShader()` values also take typed `Expr`s from the expression helpers. The schema and the generated `ShaderUniforms` / `ShaderUniformValue` types describe it, and `wgslShader()` in `@lumiscia/lumen-shared` declares the uniform struct in TypeScript and type-checks each field's value.

  The GPU renderer now keeps compiled plans within an estimated memory budget (1 GiB by default, configurable with `GpuCompositionRenderer::set_plan_cache_budget`), dropping least recently used plans instead of keeping one per animated switch or shader combination.

### Patch Changes

- d52b7d6: Allow `=` expressions in the schema and TypeScript types exactly where the engine accepts them. Paints, colours, named enums and `alpha_premultiply.mode` no longer type-check with an expression.
- b314e00: Render transformed media into the composition bounds so translation and rotation are not clipped to the source texture.
- f054aaf: Add `paint_supersample` for background and text nodes and `edge_antialias` for shape and path nodes.
- d658d6c: Emit a valid JSON schema by moving the node kind list to `x-lumen-nodeKinds`, and drop the unsupported `map` field from node types.

## 0.2.2

## 0.2.0

### Minor Changes

- 072169a: Replace legacy node property metadata with typed node params.
- cb6f4dc: Add external GPU frame targets and GPU media import plumbing for lower-copy render and encode paths.

## 0.1.0

### Minor Changes

- 906481a: Prepare the Lumen TypeScript package family for coordinated public releases.
