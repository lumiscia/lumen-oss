---
"@lumiscia/lumen-bindings": patch
---

For Rust users of `lumen-engine-ffmpeg`: `VideoDecoder::receive_rgba_frame` and `receive_cpu_frame` are removed in favour of `receive_video_frame`, and nothing in `lumen-ffmpeg` converts YUV to RGBA any more. YUV sources come out of `receive_video_frame` as planes with their colour tags, and the renderer's shader does the conversion; sources that are already RGB, gray or palette-based still come out as RGBA. `YuvVideoFrame::sample` reads the Y, Cb and Cr values at a pixel.
