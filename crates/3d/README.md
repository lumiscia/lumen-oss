# lumen-engine-3d

`lumen-engine-3d` holds the parts of Lumen's 3D scene nodes that need no GPU: primitive meshes, binary glTF models read from untrusted bytes and posed by their animations, transforms, cameras and their projections, lights, and the order and batches a scene's objects draw in. `lumen-engine` renders with it behind its `3d` feature.

This crate is experimental and tracks the renderer's needs rather than offering a standalone 3D API. World units are metres, Y is up and cameras look down -Z.

## Reading models

`Model::from_glb` is safe and bounded on any input: every length, index and count is checked against a limit before it is used or allocated for, and a bad file is an error rather than a panic. What it reads from a valid file is correct, and every number it keeps is finite. It is not a glTF validator, and accepts some files the specification forbids where they can still be read safely, such as bytes past the declared length, nodes no scene reaches that form a cycle, repeated key times, and two animation channels that target the same property, of which the later wins.

PNG and JPEG images are decoded by the `image` crate once their headers' sizes are within the limits, and a PNG reaches the decoder with only the chunks of its still image. A decoder that panics fails the model where panics unwind; on targets that abort on panic, such as `wasm32-unknown-unknown`, it aborts the process.

## Reading environment maps

`environment::decode` reads equirectangular Radiance `.hdr` files with its own reader and OpenEXR `.exr` files with the `exr` crate behind the default `exr` feature, single-threaded. Before decoding, it refuses files over 256 MiB, maps over 8192 pixels a side or 8192 × 4096 in all, and EXR files with several parts, deep data, mip or rip levels, subsampled channels, or a compression other than none, RLE, ZIP, PIZ or PXR24. A larger map is shrunk while it is read to at most 4096 × 2048, keeping its light, so the decoded map never takes more than 96 MiB. Shrinking weighs every source texel by the solid angle it shares with each smaller texel, so a map of any size keeps its light; a map of any aspect is stretched over the whole sphere. Radiance header lines are read only up to 1024 bytes each and 128 in all. Non-finite and negative texels are read as black and brighter ones are clamped, so every texel kept is finite. A bad file is an error; a decoder panic fails the map as well where panics unwind.
Compressed models need a feature each, and without it a model that requires the extension is rejected naming it:

| Feature   | Extension                    | Decoder                                       |
| --------- | ---------------------------- | --------------------------------------------- |
| `draco`   | `KHR_draco_mesh_compression` | `draco-core`, triangle meshes only            |
| `meshopt` | `EXT_meshopt_compression`    | `meshopt-pure`, every mode and filter         |
| `ktx2`    | `KHR_texture_basisu`         | `basisu`, ETC1S and UASTC transcoded to RGBA8 |
| `webp`    | `EXT_texture_webp`           | `image`'s WebP decoder                        |

Each decoder sees its input only once the reader has checked what it declares against the limits: Draco and meshopt output is charged to its own budget of 256 bytes for each byte of the file (at least 16 MiB, at most 512 MiB) before decoding, and a KTX2 or WebP image may decode to 1024 pixels, with its mips, for each of its bytes (at least 8M). `basisu` trusts a KTX2 file's offsets, so the reader checks the whole container first. What a decoder returns must then agree with the accessors and image sizes the file declares.

`lumen-engine` forwards these as `3d-draco`, `3d-meshopt`, `3d-ktx2` and `3d-webp`.

## Development

```bash
cargo check -p lumen-engine-3d
cargo test -p lumen-engine-3d
```

## Fuzzing

`fuzz/` holds two [cargo-fuzz](https://github.com/rust-fuzz/cargo-fuzz) targets, built with all four decoders and outside the workspace, so normal builds and CI leave them out. `from_glb` reads any bytes as a model; `mutated_glb` edits the JSON numbers and binary chunks of the test models instead, so every input is a well-formed `.glb`. Both pose each model that reads through its first clips and fail on anything non-finite. libFuzzer aborts on any panic, including those the reader would catch.

```bash
cd crates/3d/fuzz
mkdir -p corpus/from_glb
cp seeds/*.glb ../tests/fixtures/*.glb ../../engine/tests/fixtures/*.glb corpus/from_glb/
cargo +nightly fuzz run -O from_glb -- -fork=8 -max_total_time=600 -timeout=10
cargo +nightly fuzz run -O mutated_glb -- -fork=8 -max_total_time=600 -timeout=10
```

`-O` builds without overflow checks, as the workspace builds `basisu`: cargo-fuzz's default turns them on for every crate, and `basisu` overflows on some corrupt ETC1S streams, which wrapping keeps bounded.

`seeds/` holds two models the unit tests build, written by `cargo test -p lumen-engine-3d -- --ignored write_fuzz_seeds`. Corpora and crashes stay in `fuzz/corpus` and `fuzz/artifacts`, outside git.
