---
"@lumiscia/lumen-bindings": patch
---

A masked material (glTF `alphaMode: MASK`) hides nothing behind the texels it cuts out on every GPU. On Metal, the cut-out fragments still wrote depth, so objects drawn after them, including every blended object such as an `image_plane_3d` behind, were hidden across the whole card. Masked draws now write their depth from the shader, as do depth and normal outputs, and other draws keep their early depth test.
