---
"@lumiscia/lumen-bindings": patch
---

The browser preview works out which video frames to keep cached once per uploaded video frame instead of twice, so `setVideoFrame` and `setVideoFrameYuv` take about half the CPU time they did on a large composition with several videos.
