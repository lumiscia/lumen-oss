---
"@lumiscia/lumen-bindings": minor
---

`model_3d` reads compressed glTF models: Draco meshes (`KHR_draco_mesh_compression`), meshopt buffers (`EXT_meshopt_compression`, with its filters and `KHR_mesh_quantization`), and KTX2 textures of Basis Universal ETC1S or UASTC texels (`KHR_texture_basisu`) and WebP textures (`EXT_texture_webp`), which are used in place of a texture's PNG or JPEG fallback when it has one. Files from gltfpack and glTF-Transform load as they are. KTX2 textures are transcoded to RGBA8 on the CPU, using the mip levels they hold. Each kind of compressed data has its own limit on how far a file of its size may expand, and a file past it is rejected before it is decoded. The Lumen server and `lumen-local` read all four.
