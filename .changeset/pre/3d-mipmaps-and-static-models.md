---
"@lumiscia/lumen-bindings": minor
---

3D performance: mipmapped raster sources, and static models once their clip ends.

An `image_plane_3d` source shown smaller than its pixels, and any `model_3d` texture input larger than 64 pixels a side, is now filtered with mipmaps, trilinearly and anisotropically, instead of being sampled at full size. The mip chains are made on the GPU in the source's own format, averaging each level's texels by area in linear light, and are made once for a source that doesn't change. A source shown at its own size or larger gets none. Minified rasters look smoother, and a render with `supersample` of 1 differs slightly from before where it showed one. A chain adds a third to the raster's memory and a copy of its first level.

A `model_3d` whose animation doesn't loop is drawn once for every frame after the clip's last key (or before its first, when it holds the first pose), as it is for a model with no animation, so those frames record no 3D passes. This holds for every kind of channel, morph weights and the model's own lights and camera included. A model kept as a static frame is also drawn again when its file changes, as the media resolver reports through the model's id.
