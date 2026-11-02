---
"@lumiscia/lumen-bindings": patch
---

`kind: "video"` reads normal `.jpg` and `.jpeg` files again, and BMPs with non-zero reserved bytes. The input allowlist refused them because FFmpeg's probe picks the `image2` demuxer for those files. `InputContext::open` now allows `image2` for those extensions with filename patterns turned off, so `frame%d.jpg` can't read `frame1.jpg`. PNG, WebP, BMP, TIFF, APNG, and small JPEGs read as video open again too, by counting their frames from the file instead of failing with `unable to determine stream frame count`. `lumen-local` also renders `.mp4` output to a unique temporary file beside the target, so a failed render leaves no partial file and keeps any earlier output. It now refuses an output path that exists and is not a regular file, such as a FIFO.
