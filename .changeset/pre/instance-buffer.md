---
"@lumiscia/lumen-bindings": patch
---

`instance_3d` draws the copies of opaque surfaces at once from an instance buffer on the GPU, instead of one object per copy. 100,000 boxes under a moving camera took 24.5 ms a frame to prepare and now take 0.6 ms, and the whole frame went from 37 ms to 14 ms. Copies that move each frame only have their placements written: a ripple over the same boxes went from 72 to 50 ms. Pixels are unchanged. Blended surfaces, and those showing a raster, are still copied one by one so that they sort and mip as before.
