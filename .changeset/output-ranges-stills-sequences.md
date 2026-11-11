---
"@lumiscia/lumen-bindings": minor
---

Frame ranges, stills and image sequences, with one output description shared by `lumen-local`, the `lumen-server` library and its HTTP API.

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

`lumen-local` takes `--frames START..END`, `.jpg` outputs with `--jpeg-quality`, and an output name with `%d` (`frames/shot_%d.png`) for an image per frame. An MP4 of a range starts at time 0 with its audio cut to the same span.

`POST /renders` takes an optional `output`: `{ "format": "png_sequence", "frames": { "start": 0, "end": 120 } }`. `format` is `mp4` (the default), `mov`, `webm`, `png`, `jpeg`, `png_sequence` or `jpeg_sequence`; `jpeg_quality` is 1 to 100. The artifact has the format's `Content-Type` and filename (`.mp4`, `.mov`, `.webm`, `.png`, `.jpg`, or a stored `.zip` of `frame_0000.png` files), and `quality`, `codec`, `bit_depth` and `alpha` apply to the video formats (`mp4`, `mov`, `webm`) only. Sequences are bounded by the new `--max-sequence-frames` (3600) and `--max-sequence-bytes` (512 MiB). Requests without `output` behave as before. The SDK's `render` takes `output` and exports `outputFile(output)`.
