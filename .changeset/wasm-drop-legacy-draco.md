---
"@lumiscia/lumen-bindings": patch
---

Drop `draco-core`'s `legacy_bitstream_decode` feature from `lumen-engine-3d`, reducing the full browser WebAssembly binary by 32.6 KB raw (~7.1 KB brotli-11). Standard glTF 2.0 `KHR_draco_mesh_compression` 2.2+ models using EdgeBreaker standard (type 0) or valence (type 2) traversal are unaffected; pre-2.2 (pre-2018) Draco bitstreams and EdgeBreaker predictive traversal (type 1, regardless of bitstream version) are no longer decoded.
