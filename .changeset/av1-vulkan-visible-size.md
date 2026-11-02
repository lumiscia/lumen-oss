---
"@lumiscia/lumen-bindings": patch
---

`av1_vulkan` output now keeps the requested size in players that honour container cropping. AV1 has no crop window, so hardware that pads frames to its coded alignment, such as AMD VCN 4, turned 1920×1080 into 1920×1088 and 1080×1920 into 1088×1920. The stream now declares the coded size and the MP4 crops back to the requested one with a `clap` box. `ffmpeg` decodes these files at the requested size. Chromium ignores the crop and still shows the padding, and `ffprobe` reports the coded height with `crop_bottom` alongside it. Canvases the hardware codes exactly, such as 1280×720, 1920×1088, 2560×1440 and 3840×2160, are unchanged. The crop needs FFmpeg 7.1 or newer.

`lumen-local` and `lumen-server` warn when an encode relies on this crop, and name the coded size to render at instead. `lumen-server` now accepts `av1_vulkan` through `LUMEN_VIDEO_ENCODER`, which it previously rejected as unsupported.

For Rust: `MuxedEncoder::container_crop` and `VideoEncoder::container_crop` return the new `ContainerCrop`, which holds the coded and requested sizes, when an encode relies on the crop.
