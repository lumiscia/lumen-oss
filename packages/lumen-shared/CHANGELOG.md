# @lumiscia/lumen-shared

## 0.6.0-beta.1

### Patch Changes

- cd82e6d: A node whose expressions can't change from frame to frame is kept across frames like one with literal params. `"=vec3(0, 0, 520)"` or `"=max(0.9, 0.5)"` no longer redraws its node, and everything downstream, on every frame. An expression counts as changing when it reads `frame` or `time`, calls `progress` or `tween`, or reads another node, a template prop or measured text, as an `instance_3d` already counted.

  `vec2(..)` and `vec3(..)` of plain numbers are written to a node's params as the literal `[x, y]` or `[x, y, z]`. They are still expressions, so `vec2(1, 2).add(..)` works and `String(vec2(1, 2))` is `"=vec2(1, 2)"`.

  A number written in a composition's JSON is now read exactly, so `[0.9999999999999999, 0]` compares equal to `vec2(0.9999999999999999, 0)` as it does when the vector is written as an expression.

- Updated dependencies [ce70b5c]
  - @lumiscia/lumen-types@0.6.0-beta.1

## 0.6.0-beta.0

### Minor Changes

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

- dd03a5c: `expr` can be used as a template tag. ``expr<Vec2>`vec2(0, ${lift})` `` lowers each interpolated value the way builtin arguments are lowered, so built expressions, numbers, strings and tuples go in without stripping a leading `=` by hand. Raw expressions are bracketed where they meet an operator.
- d94ba0e: Expressions gain typed conditions. `Expr` has `gt`, `gte`, `lt`, `lte`, `eq` and `neq`, which evaluate to `Expr<boolean>`, and `and`, `or` and `not` on booleans. `when(condition, then, otherwise)` builds `if(..)` for numbers, vec2 and vec3 values, colours and strings, and `node(id, 'property')` reads another node's param.
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

- 49b2878: Node inputs and outputs have a kind, `raster` or `scene`, so a scene can't be wired where a raster goes or the reverse. A connection joining a node's output to an input of another kind fails validation with `connection.wrong_kind`, naming both nodes, the input and the two kinds, and its diagnostic points at the connection. The generated definitions give each port's and output's kind as `raster` or `scene`, where they said `raster_frame`, and list the kinds as `x-lumen-portKinds`; `NodePortKind` is `"raster" | "scene"`, and `NodeOutputKind` and `NodeInputPortKind` give a node kind's output and an input's kind. In the SDK, `addNode` inputs and `connect` only take nodes whose `type` outputs what the port takes. `NodeReference<"scene">` or `NodeReference<"raster">` is a node for a port of that kind, and a plain `NodeReference` is any node; a bare id isn't checked.
- 3ab8aa0: Params can now hold three numbers, written `[x, y, z]` in JSON or as an expression, ready for the 3D nodes' `position`, `rotation` and `scale`. Expressions gain `vec3(x, y, z)`, and vec3 values work like vec2 values: they add and subtract with each other, multiply and divide componentwise or by a number on either side, negate, and interpolate in `lerp`, `tween` and `keyframes`, e.g. `"=tween(vec3(0, 0, -400), vec3(0, 0, 0), 0, 24, 'out_expo') + vec3(0, 10 * sin(time), 0)"`. Mixing a vec3 with a vec2 is an error. Template props can be declared `"type": "vec3"`, and a `wgsl_shader` vec3 uniform takes a vec3 expression. In `@lumiscia/lumen-shared` the helper is `vec3`, which returns an `Expr<Vec3Value>` that TypeScript keeps apart from colour expressions, so a vec3 can't reach a colour param or share a `lerp` or keyframe track with a colour. `prop.vec3(..)` declares a prop, and params and prop defaults take a plain `Vec3` tuple `[x, y, z]`. A three-number tuple passed to a helper still means `rgb(..)`, so write `vec3(x, y, z)` there. `vec3` is now a reserved name, so composition functions and their parameters can't use it.

### Patch Changes

- d09b553: Documentation for 3D. The `lumen-sdk`, `lumen-shared` and `lumen-preview` READMEs link to the new 3D guide, and `lumen-shared`'s shows a scene. The descriptions of a 3D material's `color` and `shadow_catcher` params, which `lumen-types` carries as doc comments and `lumen-bindings` as node metadata, now say that only an image plane has a raster to multiply by `color` or to count in `shadow_catcher`. Nothing about how a composition renders changes.
- Updated dependencies [ec34c26]
- Updated dependencies [d09b553]
- Updated dependencies [4bacce6]
- Updated dependencies [8ef4ee5]
- Updated dependencies [97c7016]
- Updated dependencies [c4d3140]
- Updated dependencies [c657075]
- Updated dependencies [c4d3f90]
- Updated dependencies [dc06fed]
- Updated dependencies [d3e4ce9]
- Updated dependencies [89568da]
- Updated dependencies [16aae16]
- Updated dependencies [b9cc4f1]
- Updated dependencies [a37b057]
- Updated dependencies [1879afe]
- Updated dependencies [c0f96e6]
- Updated dependencies [5233133]
- Updated dependencies [fe146b5]
- Updated dependencies [ac4bc51]
- Updated dependencies [0382574]
- Updated dependencies [f045a94]
- Updated dependencies [ef94bcb]
- Updated dependencies [49b2878]
- Updated dependencies [3ab8aa0]
  - @lumiscia/lumen-types@0.6.0-beta.0

## 0.5.1

### Patch Changes

- 6285ab0: Expressions gain the `sine`, `quart`, `quint`, `circ`, `elastic` and `bounce` easing families (`in_`, `out_` and `in_out_` each, e.g. `out_bounce(t)` or `tween(0, 1, 0, 30, 'in_out_elastic')`), and the functions `sign`, `atan2`, `hypot` and `remap`. `@lumiscia/lumen-shared` exports a helper for each.
- Updated dependencies [6285ab0]
- Updated dependencies [87164d8]
  - @lumiscia/lumen-types@0.5.1

## 0.5.0

### Minor Changes

- 08a87a7: Expressions gain `path_point(path_data, t)` and `path_angle(path_data, t)` for moving along a curve, e.g. `"position": "=path_point('M 0 0 C 100 0 100 100 200 100', t)"` with `"rotation": "=path_angle(..)"`. `t` is the fraction of the path's arc length, clamped to `0..=1`, so a steady `t` moves at a steady speed. `path_point` returns a vec2 on the curve; `path_angle` is the direction of travel in degrees clockwise from +x, like `rotation`, and counts whole turns instead of wrapping, so a clockwise circle ends at 360. Moves between contours add no length, and a closed contour's closing segment does. Path data is parsed like a `path` node's `data`, and each path is measured once and cached. In `@lumiscia/lumen-shared` they are `pathPoint` and `pathAngle`.
- 3861bfd: `lerp`, `tween` and `keyframes` interpolate path data, so a `path` node's `data` can morph: `"=lerp('M 0 0 L 100 0 L 50 80 Z', 'M 0 40 L 100 40 L 50 0 Z', ease('in_out_cubic', t))"`, or `"=keyframes(frame, key(0, 'M 0 0 L 100 0 L 50 80 Z'), key(30, 'M 0 40 L 100 40 L 50 0 Z'))"`. The two paths must have the same commands in the same order, with relative and absolute commands told apart and an arc's flags matching, and they interpolate point by point; otherwise the error names the first command that differs. Paths are not resampled to match. The result writes each command out, with numbers at the precision the `path` node draws with, and numbers that both paths share aren't mixed, so they don't jitter. Keys that don't interpolate, a `hold` or two keys at the same time, may switch between paths of any structure. Strings that aren't path data don't interpolate, and the error says which one is malformed. Literal paths are checked when the expression is parsed. In `@lumiscia/lumen-shared`, `lerp`, `tween` and `key` accept strings.
- 8857666: Expressions gain keyframe tracks: `keyframes(t, keys...)` or `keyframes(t, ease, keys...)`, with one `key(at, value, ease?)` per key. Values are numbers, vec2 values or colours, and colours mix premultiplied like `lerp`. A key's `ease` is how the curve leaves it toward the next key, and keys without one use the track's default, which is `'linear'`. Before the first key and after the last the value holds, and two keys at the same `at` jump.

  Easings are now values. An easing is a name such as `'out_expo'`, the new `'hold'`, which keeps the value until the next key and then jumps, or `cubic_bezier(out, in)`, which is CSS `cubic-bezier(out.x, out.y, in.x, in.y)` with `vec2` handles. `tween` takes any easing, and `ease(easing, t)` applies one to progress `t`. In `@lumiscia/lumen-shared`, `ease` is callable as `ease(easing, t)` and still holds the easing names, such as `ease.outExpo`, and `key` and `keyframes` helpers are added.

  **Breaking:** `cubic_bezier(x1, y1, x2, y2, t)` no longer parses. Write `ease(cubic_bezier(vec2(x1, y1), vec2(x2, y2)), t)` instead; the parse error says so. The TS helper changes to `cubicBezier(out, in)` with it. `ease`, `key` and `keyframes` are now reserved names, so composition functions and their parameters can't use them.

- 404f117: Renders pick their quality by preset and their codec by name. Every host defines `draft`, `standard`, `high` and `lossless`, each with settings per FFmpeg encoder, and `render_settings` in a composition gains `codec` (`h264`, `hevc` or `av1`) and `quality` (a preset name), which the `RenderSettings` type and `new Composition({ renderSettings: { codec, quality } })` carry. Templates' render settings are now `TemplateRenderSettings`, without them. Previews ignore both.

  A render with no preset now uses `standard` instead of a 14 Mb/s target. Each codec tries its encoders in order, with one warning for each that won't open: on Linux the Vulkan encoder, then `libx264`, `libx265` or `libsvtav1`, and on macOS VideoToolbox, then the same software encoders. A preset only tries the encoders it has settings for, so `lossless` uses `libx264`, `libx265` or NVENC H.264 and HEVC. `LUMEN_VIDEO_ENCODER` still pins one encoder with no fallback. On `lumen-server`, a preset, codec or pinned encoder this FFmpeg build can't serve is a `422` when the render is requested. An output container that can't hold the codec, such as H.264 in `.webm`, fails when the encoder is created.

  `LUMEN_QUALITY_CONFIG` (and `lumen-server --quality-config`) names a JSON file that sets the default preset and codec, each codec's encoder order, and presets' settings per encoder: `crf`, `qp` or `bitrate`, an encoder `preset`, and raw FFmpeg `options`. It is checked when `lumen-server` starts or `lumen-local` loads it, so an unknown field, codec, encoder or option, or a value outside an option's range, fails then instead of on a render.

  `lumen-local` gains `--quality`, `--codec` and `--quality-config`. `--bitrate`, `--crf`, `--qp` and `--preset` keep their meaning, replacing the preset on every encoder tried, and can't be combined with `--quality`. `--crf`, `--qp` and `--preset` now also work on `libx265` and `libsvtav1`.

  For Rust users of `lumen-engine-ffmpeg`, `QualityConfig` loads and resolves presets into a `VideoEncoderChoice`, which holds the ordered `EncoderCandidate`s a render tries; `EncoderQuality` gains raw `options`, and its `rate_control` is optional, so `EncoderQuality::default()` and `VideoEncoderConfig::cpu_rgba` leave rate control at the encoder's default. `VideoEncoderChoice::resolve` and `check_quality` are gone. In `lumen-server`, `RenderOptions::encoder` and `resolve_encoder` replace `video_encoder` and `quality`, and `Mp4Render::open` takes the choice.

### Patch Changes

- 92b28e3: Every package now has a README, so its npm page describes it.
- Updated dependencies [6cddffa]
- Updated dependencies [db420ff]
- Updated dependencies [d21228a]
- Updated dependencies [4df3509]
- Updated dependencies [428e0a7]
- Updated dependencies [8a28aa1]
- Updated dependencies [08a87a7]
- Updated dependencies [3861bfd]
- Updated dependencies [53e1e41]
- Updated dependencies [3665a2c]
- Updated dependencies [b7342d2]
- Updated dependencies [f31d22c]
- Updated dependencies [8857666]
- Updated dependencies [48ad23c]
- Updated dependencies [c491e28]
- Updated dependencies [92b28e3]
- Updated dependencies [1d76f93]
- Updated dependencies [404f117]
- Updated dependencies [ef422d1]
- Updated dependencies [5db9958]
  - @lumiscia/lumen-types@0.5.0

## 0.4.0

### Minor Changes

- c1c57b2: `addNode` takes `inputs`, so a node connects to the nodes that feed it where it is added: `addNode({ type: "merge", inputs: { base: background, overlay: title } })`. A variadic port such as `layers` takes an array, `{ layers: [background, image, title] }`. Ports are typed from the node kind, and the new generated `NodeVariadicInputPortName<Kind>` marks the ones that take arrays. The connections are the ordinary ones, in the order written, so the JSON is the same as with `connect`, which stays. `literal(text)` escapes a leading `=` as `==`, and `expr("if(frame > 30, 1, 0)")` wraps raw expression text, such as `if(..)` and `node(..)`, as an `Expr`.

  Breaking: `Composition` loses `setSize`, `setFps`, `setDurationFrames`, `setDurationSeconds`, `setMetadata` and `setAudio`; pass `renderSettings`, `timeline` and `metadata` to the constructor. `connect` loses `fromPort`, which the engine only accepts as `output`. `connect` and `inputs` throw for a node handle from another composition; numeric ids are left to the engine. A failed `addNode` leaves the composition, including its templates, functions and next automatic id, as it was. A composition definition no longer has `.composition`. `moves(base, [{ by, .. }])` emits `by * tween(0, 1, ..)` instead of `tween(0, by, ..)`, with the same value, and `Expr` no longer tracks a kind. `fn` and `addFunction` no longer check names and `wgslShader` no longer checks uniform names or types; the engine rejects function names when the composition loads, and the shader compiler reports bad uniform names and types. A `toJSON` on a plain array in a param, or a non-enumerable one on a plain param object, is no longer called: `{ text: "draft" }` with a hidden `toJSON` returning `{ text: "final" }` now serializes as `draft`. An enumerable `toJSON` on a plain object still runs, and so does one on a class instance or array subclass.

- b6ece6b: Composition errors now name the JSON path, such as `timeline.fps: ...`, and more mistakes fail instead of being ignored. Unknown keys in `timeline` and `render_settings` are errors, including in templates, so a typo like `fpss` no longer falls back to 30 fps. Other unknown keys at the root and in `audio` are still ignored.

  `render_settings.background_color`, which rendering never read, is an error: connect a `background` node instead. `Composition.setBackgroundColor` is gone. Audio clips need whole-number `start_ms` and `duration_ms`, and `source_start_ms` must be a whole number when set; `start_frame`, `duration_frames` and `source_start_seconds` are errors. `schemaVersion` is ignored and the schema no longer types it, `lumenSchemaVersion` must be absent or `0.1.0` in the schema too, and a non-string `$schema` or a `metadata` that isn't an object with a string `name` now fails at load, as it already did in the schema. The builder no longer writes `durationSeconds` into `timeline`. `CompositionBuilder::build` now rejects fps that isn't positive and finite, a zero size or duration, and a prop default of the wrong kind, for the root and templates.

- 6b3a821: `Composition.connect()` takes the target port explicitly instead of defaulting to `input`, which no node has, so a connection without `toPort` no longer builds a composition the engine rejects. Given the target node, `toPort` is typed to its inputs, from the new generated `NodeInputPortName<Kind>` type: `composition.connect(background, merge, { toPort: "base" })` type-checks, and `toPort: "source"` on a `merge` does not. `addNode` now infers the node kind it returns.
- 0003560: Expressions fail earlier and accept more number forms. An unknown variable such as `=frmae + 1` is now rejected when the composition loads instead of failing mid-render, an unknown string escape such as `'\q'` is an error rather than dropping the backslash, and `node(id)` rejects ids above 2^53. Numbers may now use exponents and a leading dot (`1e3`, `1.5e-3`, `.5`), and a number too large to represent is rejected rather than becoming infinity. The SDK's helpers write large and small numbers in exponent form as a result.

  The `linear(a, b, t)` builtin and the SDK's `linear` helper are gone; use `lerp`. `'linear'` stays as an easing name. `x-lumen-expressions` in the schema now lists `forms` (`if` and `node`) and `reserved` names, which `@lumiscia/lumen-types` exports as `expressionForms` and `expressionReservedNames`.

- 8eac30a: Audio has one API, `Composition.addAudio`, with every time in seconds: `composition.addAudio({ source: "music", start: 0.5, duration: 2.5, offset: 1.25, trackId: "music", gain: 0.8 })`. Seconds become the whole milliseconds the composition JSON uses, whatever the composition's fps. A clip's track is declared as a plain track if it isn't one yet, `trackId` defaults to `main`, and clips may overlap. `addAudioTrack({ id, name, muted, solo, gain })` sets a track's mute, solo and gain; call it before the track's first clip. `AudioTrack`, its `addClip`, and the `startMs`, `startSeconds`, `durationMs`, `durationSeconds`, `sourceId`, `sourceStartMs` and `sourceStartSeconds` clip options are gone, as are the builder's overlap check and its `AudioTimelineInput`, `AudioClipInput` and `AudioTrackInput` types. `@lumiscia/lumen-sdk` no longer re-exports `AudioTrack`.
- 7c5ccae: `Composition.addNode` takes typed `Expr` values inside rich text, not just top-level params: span fields in `content`, and the fields of `underline`, `strikethrough` and `highlight` at node and span level (`{ extent: vec2(0, progress(0, 30)) }`). Any nested field that accepts an `=...` string accepts an `Expr` of the matching type, and the new `ExpressionInput<T>` type applies the same mapping to a value built outside `addNode`, such as `ExpressionInput<TextSpan>`.
- fb4c25b: Compositions now fail to load instead of silently rendering something else. These are errors: `params` that isn't an object, unknown keys on nodes and connections (such as `properties` for `params`), duplicate node ids within a graph, repeated audio track or clip ids, audio clips on a track the timeline doesn't declare, and non-finite or negative audio gains. The audio checks also apply to compositions built in Rust. `lumen-server` and WASM parse errors now include the nested cause, such as the bad param inside a template, not just `composition `card``. `AudioTrack.addClip` now gives clips without an id the default `<track id>-clip-<n>`, so clips on different tracks don't collide.

  Media now renders correctly. A canvas-sized `media_in` wired straight to `media_output` writes to external targets, where GPU encodes previously got an empty frame. Media that changes size renders at its own size instead of stretched into the old one. Replacing an image in a `LumenMediaStore` with one of the same size shows the new pixels. Cycling one `media_in` through several same-size images uploads each once instead of on every switch; images no plan is showing are freed, least recently used first, once they exceed 256 MiB.

  For Rust: `MediaTextureKey` loses `frame`, `MediaTextureUpload::key` and `BoundFrame::use_media_texture` take `Option<MediaTextureKey>`, with `None` for video frames, and `GraphValidationError::InvalidAudio` and `MediaError::UnknownAudioTrack` are new. `texture_from_metal` takes an optional drop callback, and `GpuCompositionRenderer` gains `set_media_texture_cache_budget`.

- 37500c3: The text node's `content` takes styled spans as well as a string. Each span's font family, size, weight, style, letter spacing and color override the node's, and anything a span leaves out comes from the node. Span text and every span field can be an `=expression`. String content, including `=expressions`, renders as before. Span decorations parse but are not drawn yet.

  Each span is drawn in its own `color` paint. The new `paint_space` param (`span` by default, or `text`) sets whether a span's own gradient covers just that span or the whole text. The node's `color`, which spans without a color use, always covers the whole text. `paint_supersample` still applies to the whole node.

  The `glyph_*` params can read the new `span` and `span_count` expression globals, so they can target a single span, as in `=if(span == 1, vec2(0, -10 * sin(time * 6 + glyph)), vec2(0, 0))`. `@lumiscia/lumen-shared` exports them as `span` and `spanCount`, and a text node's `content` takes a string `Expr`. `text_width(node(id))` and `text_height(node(id))` measure the spans, and `node(id, "content")` reads span content as its joined text when that text is literal.

  For Rust: `TextParams::content` is now a `TextContent`, and `TextParamsDelegate::content` is now a `TextContentDelegate`, so wrap string content in `TextContentDelegate::Text`. `GpuGlyphInstance` packs a paint index into the upper bits of `mode` (read and set with `render_mode()`, `paint()` and `set_paint()`), and `GpuTextGlobals` no longer has `paint_bounds`, since each paint now carries its own bounds.

### Patch Changes

- Updated dependencies [c1c57b2]
- Updated dependencies [b6ece6b]
- Updated dependencies [6b3a821]
- Updated dependencies [0003560]
- Updated dependencies [75572c3]
- Updated dependencies [eb354e6]
- Updated dependencies [4f3894f]
- Updated dependencies [93fb454]
- Updated dependencies [5a0939c]
- Updated dependencies [37500c3]
- Updated dependencies [f35c132]
- Updated dependencies [0d47151]
- Updated dependencies [e3d02e6]
- Updated dependencies [ca87eaa]
  - @lumiscia/lumen-types@0.4.0

## 0.3.1

### Patch Changes

- @lumiscia/lumen-types@0.3.1

## 0.3.0

### Minor Changes

- 3f45d73: The GPU renderer now renders unchanging parts of a composition once and reuses them: a node whose params hold no expression and that reads no media or shader time, fed only by such nodes, is neither bound nor dispatched again on later frames of its plan. Output frames are unchanged. Kept output lives in the plan's own textures, so it counts against the plan cache budget and is dropped with its plan, and recompiles, font loads and texture replacements render it again.

  `memo` now caches. Its `cache_id` and `allow_expressions` params are removed in favour of `key`, an expression evaluated at the frame being rendered: while it gives the same value as on the frame `memo` last rendered, `memo` shows that output and nothing only it reads is bound or dispatched. `"key": "=floor(frame / 4)"` runs a heavy shader on one frame in four, and `"=floor(frame / 2)"` animates on twos. What `memo` holds is the source as rendered on the first frame of each key value that is rendered, so pair it with a `time_remap` to `floor(frame / 4) * 4` when frames may render out of order. The snapshot belongs to the memo node and its time-remap path rather than to a plan, so fractional frames and upstream switches that bind another plan show it too; it is taken again once every plan holding it is evicted, and any edit to the composition drops it, since a key holds output across frames, never across edits. Bounds a `crop` publishes are kept with the pixels. A literal or missing `key` adds nothing to the caching above. From TypeScript, `composition.addNode({ type: "memo", params: { key: floor(frame.div(4)) } })`.

  `GpuCompositionRenderer::cached_pass_count` and `submitted_pass_count` report how many passes were skipped and submitted, and `lumen-local` prints the per-frame averages as `static_cached` and `passes`.

- 076bf72: Text nodes take `glyph_offset`, `glyph_scale`, `glyph_rotation` and `glyph_opacity`, evaluated for every glyph to stagger glyphs, words and lines apart. Inside them, expressions can read the `glyph`, `word` and `line` index of the glyph being drawn and the `glyph_count`, `word_count` and `line_count` of the text, such as `"glyph_opacity": "=tween(0, 1, 10 + glyph * 2, 12)"`; elsewhere those globals are an error. Each glyph scales and turns about the middle of its advance and line, before the text's own transform, and fades before its `opacity`. `@lumiscia/lumen-shared` exports them as `glyph`, `word`, `line`, `glyphCount`, `wordCount` and `lineCount`.
- b0bbf52: Default new compositions to 30 fps in the TypeScript builder, matching the engine, and convert audio `start_frame`/`duration_frames` at the composition's frame rate instead of a fixed 30. Compositions that relied on the old 24 fps builder default now play faster (240 frames last 8 s instead of 10 s); call `setFps(24)` to keep the old timing.
- 1ab946d: Add typed expression helpers that build `=...` expression strings: one helper per expression builtin (`tween`, `vec2`, `rgb`, `spring`, `outExpo`, ...), `ease.*` easing names, `sec`/`ms` units, `hex` colours, the `frame`/`time`/`fps`/`width`/`height` globals, and `.add`/`.sub`/`.mul`/`.div`/`.oneMinus`/`.clamp` with the evaluator's vec2 and colour rules. `fn("rise(start)", ...)` defines a composition function, which `Composition.addFunction` adds to `functions`, and `moves(base, [...])` sums tweens. Node params accept these expressions directly.
- 4e61e7a: `wgsl_shader` now takes up to 16 inputs (`input0..input15` plus a shared `input_sampler`), gives every shader `lumen.time`, `lumen.frame` and `lumen.resolution`, binds typed uniforms by name from the shader's uniform struct, rebuilds its pipeline when the source changes, premultiplies straight-alpha inputs before the shader sees them, and rejects unknown or malformed uniform values and shader bindings above 2 instead of ignoring them.

  Uniforms are set with a `uniforms` object instead of the `bindings` string, which is removed. Each value is a number, an array of 2-4 numbers or `=expression`s, a whole-value `=expression`, or, for `vec3<f32>` and `vec4<f32>` fields, a `#rrggbb` / `#rrggbbaa` colour or a colour expression such as `=rgb(255, 128, 0)`. `wgslShader()` values also take typed `Expr`s from the expression helpers. The schema and the generated `ShaderUniforms` / `ShaderUniformValue` types describe it, and `wgslShader()` in `@lumiscia/lumen-shared` declares the uniform struct in TypeScript and type-checks each field's value.

  The GPU renderer now keeps compiled plans within an estimated memory budget (1 GiB by default, configurable with `GpuCompositionRenderer::set_plan_cache_budget`), dropping least recently used plans instead of keeping one per animated switch or shader combination.

### Patch Changes

- 38a05a8: Prevent explicit and automatically allocated composition node IDs from colliding.
- Updated dependencies [2c68ade]
- Updated dependencies [5726783]
- Updated dependencies [577d694]
- Updated dependencies [d52b7d6]
- Updated dependencies [b314e00]
- Updated dependencies [3f45d73]
- Updated dependencies [076bf72]
- Updated dependencies [2b50b73]
- Updated dependencies [f054aaf]
- Updated dependencies [97f55bf]
- Updated dependencies [1b30cb7]
- Updated dependencies [d658d6c]
- Updated dependencies [b88aeda]
- Updated dependencies [4e61e7a]
  - @lumiscia/lumen-types@0.3.0

## 0.2.2

### Patch Changes

- @lumiscia/lumen-types@0.2.2

## 0.2.0

### Minor Changes

- 072169a: Replace legacy node property metadata with typed node params.
- cb6f4dc: Add external GPU frame targets and GPU media import plumbing for lower-copy render and encode paths.
- cb6c9fa: Add composition audio track and clip builder helpers with canonical timeline output.

### Patch Changes

- Updated dependencies [072169a]
- Updated dependencies [cb6f4dc]
  - @lumiscia/lumen-types@0.2.0

## 0.1.0

### Minor Changes

- 906481a: Prepare the Lumen TypeScript package family for coordinated public releases.

### Patch Changes

- Updated dependencies [906481a]
  - @lumiscia/lumen-types@0.1.0
