# @lumiscia/lumen-cli

## 0.6.0-beta.4

### Patch Changes

- @lumiscia/lumen-types@0.6.0-beta.4

## 0.6.0-beta.3

### Minor Changes

- dea8cf4: **A composition no longer holds encoding settings.** `codec`, `quality`, `bit_depth` and `alpha` are gone from `render_settings`, and a document that still has them is rejected as an unknown field. How a video is encoded is decided by the render request, then by the codec the output container holds (`prores` in a `.mov`, `vp9` in a `.webm`), then by the host's defaults. Every surface already took these options, so move them there; `lumen render comp.json -o out.mov` with no flags still picks ProRes.

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

- 7573c49: The SDK's API types are the generated ones from `@lumiscia/lumen-types`, and the client gains the three routes it lacked.

  `Render` is a union on `status`. `waitForRender` returns a `SucceededRender`, so its `url` is a `string`, and a `failed` or `cancelled` render has `error`. A render's `progress` is a number, 0 when the API sends none, and `durationMs` and `outputBytes` are new:

  ```ts
  // Before
  const done = await lumen.waitForRender(render);
  await fetch(done.url!);

  // Now
  const done = await lumen.waitForRender(render); // SucceededRender
  await fetch(done.url);
  if (render.status === "failed") console.error(render.error.code); // FailedRender
  ```

  `TemporaryMedia` is now `Media`, and `parseMedia` rejects a body that lacks one of its fields with `invalid_response`. `RenderFailure.retryable` is always set. `MediaKind`, `MediaMetadata`, `MediaStatus`, `MediaSource`, `OutputFormat`, `FrameRange` and `RenderOutput` are the generated types, re-exported. A media source that fails a render or a request is on `LumenError.media`.

  `check`, `getMedia` and `deleteMedia` are new:

  ```ts
  const report = await lumen.check(composition, { media }); // CheckReport: { ok, diagnostics }
  const upload = await lumen.getMedia("asset:m_…"); // the id may carry `asset:`
  await lumen.deleteMedia(upload);
  ```

  `render` and `check` accept a `CompositionSource`: a `LumenComposition`, or any object with `toJSON()`. A `Composition` from another copy of the package works too. `CompositionSource` is exported from `@lumiscia/lumen-types`.

  `CheckResponse` is now `CheckReport`, in the `lumen-server-api` crate and the generated types. `@lumiscia/lumen-cli` exports the generated `CheckReport`, `CompositionSource` and `FrameRange` instead of its own copies.

### Patch Changes

- Updated dependencies [a822c9b]
- Updated dependencies [0b4d4bc]
- Updated dependencies [b050d65]
- Updated dependencies [dea8cf4]
- Updated dependencies [cf41f99]
- Updated dependencies [28eea4f]
- Updated dependencies [2a50568]
- Updated dependencies [3a7cd1a]
- Updated dependencies [edc2aa0]
- Updated dependencies [7573c49]
  - @lumiscia/lumen-types@0.6.0-beta.3

## 0.6.0-beta.2

### Minor Changes

- eec0701: **New: `lumen preview` writes a contact sheet of a composition.** It renders frames sampled evenly across the timeline and tiles them into one `.png` or `.jpg`, so a whole composition can be seen at a glance without rendering a video.

  ```bash
  lumen preview composition.json -o sheet.png
  lumen preview composition.json -o sheet.jpg --count 20 --columns 5 --frames 30..300
  ```

  `--count` (default 12) frames are spread from the first frame of `--frames` (default the whole timeline) to its last, `--columns` (default 4) to a row on a sheet `--width` (default 1920) pixels wide, each cell at the composition's aspect ratio over a dark grey background. It also takes `--media-root`, `--font-dir` and `--max-text-glyphs`, as `lumen render` does.

- 29827be: `@lumiscia/lumen-cli` can be imported to check, render and preview a composition from a script, so running the script that builds it is all it takes. It works under Node, Bun and Deno, pipes the composition to the installed `lumen` binary and passes the binary's progress and warnings through:

  ```ts
  import { check, preview, render } from "@lumiscia/lumen-cli";

  await render(composition, { output: "out.mp4", quality: "high" });
  await preview(composition, { output: "sheet.png", count: 20, columns: 5 });
  const { ok, diagnostics } = await check(composition);
  ```

  `composition` is the JSON or anything with `toJSON()`, such as the SDK's `Composition`. A failed run rejects with a `LumenCliError` holding the exit code and stderr, and `quiet` and `signal` hide the output and stop the run. `preview()` needs a `lumen` binary with `lumen preview`.

- 7a3a9e9: `lumen render` and `lumen check` read the composition from stdin when it is `-`, so a script can pipe one in without writing a file:

  ```bash
  node composition.js | lumen render - -o out.mp4
  cat composition.json | lumen check - --format json
  ```

  Messages and the `lumen check` summary call the source `<stdin>`. Media is found under `--media-root`, which defaults to the current directory. Empty input, or stdin that is a terminal, fails with an error instead of waiting. A file named `-` is reached as `./-`.

### Patch Changes

- @lumiscia/lumen-types@0.6.0-beta.2

## 0.6.0-beta.1

### Minor Changes

- b8fef96: **Breaking: `lumen` now has two commands, `render` and `check`, and takes the composition as a positional argument.** The `--composition` and `--check` flags are gone, with no aliases, and `--output` has a short form, `-o`.

  Before:

  ```bash
  lumen --composition composition.json --output out.mp4 --codec hevc
  lumen --composition composition.json --output frame.png --frame 30
  lumen --composition composition.json --check --format json
  ```

  After:

  ```bash
  lumen render composition.json -o out.mp4 --codec hevc
  lumen render composition.json -o frame.png --frame 30
  lumen check composition.json --format json
  ```

  Each command takes only its own flags: `check` has `--format`, `--media-root` and `--font-dir`, and rejects the video, image and `--max-text-glyphs` flags it used to ignore, while `render` has no `--format`. The other flags and the environment variables keep their names and meanings. Bare `lumen` prints the two commands.

  `lumen render -h` and `lumen check -h` now print a line per flag, grouped into general, video and image options, and `--help` a little more. The reference that the help used to print in full (encoder order, which encoders take `--crf`, `--qp` and `--preset`, sequence numbering, environment variables) is in the [CLI README](https://github.com/lumiscia/lumen-oss/tree/main/crates/cli#readme), which the help points to.

## 0.6.0-beta.0

### Minor Changes

- 60b7798: **`lumen-local` is now `lumen`, installed from npm.** `npm install -g @lumiscia/lumen-cli` installs a prebuilt `lumen` binary for Linux on x64 and arm64 (glibc 2.28 or later) and for macOS on Apple silicon. FFmpeg 9 is linked into it with `libx264`, `libx265` (8-bit and 10-bit), `libsvtav1`, `libvpx-vp9`, `libopus` and `libdav1d`, plus the Vulkan and NVENC encoders on Linux, where NVENC takes frames through CUDA, and VideoToolbox on macOS, so nothing else needs installing. Those binaries are distributed under GPL-3.0-or-later because of x264 and x265; the source stays Apache-2.0.

  **Breaking:** the crate moves from `crates/local` to `crates/cli` and is named `lumen-cli`, and its binary is `lumen`, so `cargo run -p lumen-local --` becomes `cargo run -p lumen-cli --`. Flags and environment variables are unchanged. Errors now start with `lumen failed`.

  **AV1 uses NVENC on NVIDIA.** On a Linux host with NVIDIA's encode library, `av1` now tries `av1_nvenc`, then `libsvtav1`, instead of `av1_vulkan`, which writes wrong frames on NVIDIA. Other hosts and codecs keep their order.

  **Fixed:** `hevc_nvenc` failed to open with a `crf` or a built-in preset, because `maxrate` was set as an option of the encoder and is one of the codec context. `lumen-engine-ffmpeg` also builds against FFmpeg 9 now, which removed `AVCodec::sample_fmts`.

### Patch Changes

- 0abbc00: The platform packages of the `lumen` CLI now carry the GPL-3.0 text as `LICENSE` and, in `licenses/`, the notices of every Rust crate in the binary and the license of the built-in Roboto font.
