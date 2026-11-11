# Fox.glb

The glTF sample fox, from the Khronos glTF sample models, used unchanged by the `model_3d` render
tests.

- Model: PixelMannen, <https://opengameart.org/content/fox-and-shiba>, CC0.
- Rigging and animation: @tomkranis,
  <https://sketchfab.com/3d-models/low-poly-fox-by-pixelmannen-animated-371dea88d7e04a76af5763f2a36866bc>,
  CC BY 4.0.
- Conversion to glTF: @AsoboStudio and @scurest,
  <https://github.com/KhronosGroup/glTF-Sample-Models/pull/150#issuecomment-406300118>, CC BY 4.0.

Licensed under CC BY 4.0, <https://creativecommons.org/licenses/by/4.0/>.

`Fox-draco.glb` and `Fox-meshopt-ktx2.glb` are compressed copies, made from this directory with the
tools `crates/3d/tests/fixtures/README.md` names:

```sh
gltf-transform draco Fox.glb Fox-draco.glb
gltfpack -i Fox.glb -o Fox-meshopt-ktx2.glb -cc -vtf -tc
```
