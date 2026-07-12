# lumen-engine-text

`lumen-engine-text` contains text layout and glyph data helpers used by the renderer.

This crate tracks Lumen's renderer needs rather than presenting a standalone text engine API.

## Platform Notes

This crate itself does not require a GPU, but its output is designed to feed GPU render paths in `lumen`.

## Development

```bash
cargo check -p lumen-engine-text
cargo test -p lumen-engine-text
cargo test -p lumen-engine-text --features experimental-msdf # compatibility feature name
```

## Alpha text sharpness

The production alpha path previously discarded Cosmic Text's physical glyph position. Cosmic Text selects a Swash raster whose cache key encodes quarter-pixel x/y bins and supplies the integer pixel position at which that raster should be drawn. Lumen instead forced the cache key to the zero subpixel bin and placed the glyph quad at the original fractional layout coordinate. Linear atlas sampling then interpolated a mask that was already antialiased, effectively filtering it twice. Increasing the video resolution did not remove that sampling mismatch, which is why 1080p output could still look soft.

Lumen now keeps the physical cache key and draws the mask at Cosmic Text's integer physical position. Layout measurement is unchanged. Fractional origins passed through text layout select the corresponding subpixel raster while the quad remains pixel aligned; engine-level animation remains unsnapped and is applied afterward as an instance offset. Controlled tests cover cache-key selection, integral instance placement, distinct raster bytes at different fractional origins, unchanged measurement, and fractional animation behavior.

## Production hybrid MSDF path

Hybrid MSDF is enabled by default and is the default `text` node renderer. Compositions can set `render_mode: "raster"` for a forced CPU-raster path. The `experimental-msdf` Cargo feature name remains as a compatibility switch for downstream crates, but it is now part of the default feature set.

The original prototype scaled its generation data with glyph occurrences rather than unique glyphs. Repeated large characters duplicated jobs, outline segments, and a per-pixel job-index array; large glyph area multiplied by text count could exhaust the pixel budget or allocate very large buffers. The fixed atlas also charged MSDF range padding twice, and the outline-job cache had no bound across animated sizes/configurations. A small still frame could therefore look correct while a realistic sequence fell back, lost glyphs, or grew memory.

The production path:

- owns a persistent CPU/GPU atlas per compiled text node instead of sharing process-global frame state;
- generates each outline once at a canonical 96 px size and scales its quad at draw time, so size and position animation reuse the same field;
- prepares each unique MSDF or raster glyph once and reuses its atlas entry across occurrences and frames;
- removes the occurrence-sized per-pixel job map, with compute work resolving compact job pixel ranges;
- selects signed true distance independently per color channel, using orthogonality to resolve shared-corner ties before converting the selected edge to pseudo-distance;
- stores an additional true-distance channel and applies nonzero-winding sign correction using half-open flattened curve crossings, avoiding whole-row inversions at curve extrema and compound contours;
- uses a single-sample derivative-based coverage calculation in the draw shader, reducing aliasing without supersampling or extra atlas reads;
- reserves only filtering isolation around an MSDF field instead of charging its distance range twice;
- keys outline jobs by glyph and generation configuration;
- bounds the LRU-style CPU outline cache to 256 entries and 65,536 segments;
- uses indirect compute dispatch and writes a zero dispatch on unchanged frames, eliminating cached-glyph generation work;
- performs bounded generational atlas eviction on capacity pressure, rebuilding only the current working set into the existing node-owned GPU texture;
- enforces explicit glyph, segment, MSDF-pixel, and atlas limits, falling back to alpha rasterization when MSDF generation is unsuitable;
- preserves color-raster fallback for glyphs that are not represented by the outline path.

This bounds CPU and GPU growth. Stable frames retain their atlas and only update instance/paint data; content churn appends new glyphs until capacity is reached, then atomically starts a new bounded atlas generation. Recompiling or recreating the renderer rebuilds the node-owned resources and cache together.

### Benchmark snapshot

These release-mode results were collected from the text benchmark's 20-frame scenario matrix with a 4096² atlas for the large scenarios. Times are mean CPU layout plus atlas/job preparation per frame; they do not include GPU compute dispatch, drawing, readback, encoding, or presentation.

| scenario                        | raster prep | GPU-MSDF prep | workload                              |
| ------------------------------- | ----------: | ------------: | ------------------------------------- |
| baseline, 64 px / 1080p         |    275.7 µs |      235.0 µs | short reference text                  |
| large, 240 px / 1080p           |     2.58 ms |       1.05 ms | repeated large glyphs                 |
| dense, 96 px / 4K               |     4.62 ms |       1.77 ms | 2,230 glyphs                          |
| subpixel motion, 160 px / 1080p |     2.71 ms |       1.07 ms | fractional frame-to-frame origin      |
| glyph churn, 128 px / 4K        |     4.50 ms |       1.32 ms | 1,712 glyphs with changing frame text |

Peak process RSS was approximately 94–96 MiB in the 4096² matrix runs. A separate 1,800-frame MSDF glyph-churn soak rendered 1,712 of 1,712 laid-out glyphs on every frame with no capacity loss. It averaged 1.37 ms CPU preparation, reached 64.2 MiB of frame working data and 6.9 MiB of used atlas data, generated at most 73 jobs / 382,985 MSDF pixels, and peaked at 78.8 MiB RSS.

Use these numbers as a regression baseline, not an end-to-end comparison. The benchmark reports laid-out versus rendered glyph ranges, working bytes, used atlas bytes, MSDF jobs/pixels, and peak RSS precisely so a fast run that silently falls back or drops glyphs is visible. See [`lumen-bench`](../bench/README.md) for scenarios, commands, and repeatability guidance.

### End-to-end production snapshot

The matched 1080p animated stress compositions exercise the real engine path, persistent atlas, GPU generation/draw, compositing, readback, and encoding. These single-host release results are regression data, not universal hardware claims.

| workload                             | hybrid MSDF | forced raster |
| ------------------------------------ | ----------: | ------------: |
| 20-frame timestamped render profile  |   151.3 fps |     122.5 fps |
| text draw GPU time                   |     1.16 ms |       0.46 ms |
| atlas generation GPU time, amortized |     0.44 ms |       0.01 ms |
| 120-frame CPU-encoded video          |    98.5 fps |      74.3 fps |
| 1,800-frame render-only MSDF soak    |   225.3 fps |             — |

The MSDF draw is more expensive than sampling a raster mask, while canonical-size reuse avoids per-frame glyph uploads and dominates this animated workload. GPU timestamp collection itself synchronizes and adds readback overhead; use `render-only` for throughput.

### Portability and recovery coverage

- Linux CI runs the full Rust workspace with a Vulkan software adapter; Windows CI compiles the production path and runs CPU atlas/cache tests.
- The visual gallery accepts `LUMEN_TEXT_<SCRIPT>_FONT_PATH` and `_FONT_FAMILY` overrides for Arabic, Hebrew, mixed bidi, Devanagari, CJK, Thai, combining marks, Greek/Cyrillic, ligatures, and emoji fixtures on any host.
- `LUMEN_TEXT_VARIABLE_FONT_PATH` and `LUMEN_TEXT_VARIABLE_FONT_FAMILY` add a matched variable-font gallery scene.
- `GpuCompositionRenderer::recover_gpu_resources` recreates the device and every compiled plan resource, invalidates node-local atlas metadata, and repopulates text on the next frame; a GPU test verifies identical output after recovery.
- Tests cover malformed font input, color fallback, forced raster mode, capacity eviction, long animated preparation, renderer/resource recovery, 32–600 px scaling, and hard inversion/line detection.
