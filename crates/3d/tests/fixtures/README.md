# Compressed model fixtures

`grid.glb` is written by `grid.py` (Python 3, standard library only): a 9 × 9 grid with normals,
texture coordinates, a two-joint skin, a clip named `bend` and a 16 × 16 PNG texture. The other
files are compressed copies of it, made from this directory with
[glTF-Transform](https://gltf-transform.dev) CLI 4.5.1 (`npm install @gltf-transform/cli@4.5.1`) and
the gltfpack 1.3 binary from <https://github.com/zeux/meshoptimizer/releases/tag/v1.3>
(`gltfpack-ubuntu.zip`; the npm build of gltfpack lacks Basis Universal and WebP):

```sh
python3 grid.py
gltf-transform draco grid.glb grid-draco.glb
gltfpack -i grid.glb -o grid-meshopt.glb -cc -vtf
gltfpack -i grid.glb -o grid-etc1s.glb -noq -tc
gltfpack -i grid.glb -o grid-uastc.glb -noq -tu
gltfpack -i grid.glb -o grid-webp.glb -noq -tw
```

`grid-meshopt.glb` uses the attribute and triangle modes, the octahedral, quaternion and
exponential filters, a fallback buffer and `KHR_mesh_quantization`. gltfpack resamples the clip to
31 keys.
