# lumen-engine-text

`lumen-engine-text` contains text layout and glyph data helpers used by the renderer.

This crate is experimental and currently tracks Lumen's renderer needs rather than a standalone text engine API.

## Platform Notes

This crate itself does not require a GPU, but its output is designed to feed GPU render paths in `lumen`.

## Development

```bash
cargo check -p lumen-engine-text
cargo test -p lumen-engine-text
cargo test -p lumen-engine-text --features experimental-msdf
```

## Alpha text sharpness

The production alpha path previously discarded Cosmic Text's physical glyph position. Cosmic Text selects a Swash raster whose cache key encodes quarter-pixel x/y bins and supplies the integer pixel position at which that raster should be drawn. Lumen instead forced the cache key to the zero subpixel bin and placed the glyph quad at the original fractional layout coordinate. Linear atlas sampling then interpolated a mask that was already antialiased, effectively filtering it twice. Increasing the video resolution did not remove that sampling mismatch, which is why 1080p output could still look soft.

Lumen now keeps the physical cache key and draws the mask at Cosmic Text's integer physical position. Layout measurement is unchanged. Fractional origins passed through text layout select the corresponding subpixel raster while the quad remains pixel aligned; engine-level animation remains unsnapped and is applied afterward as an instance offset. Controlled tests cover cache-key selection, integral instance placement, distinct raster bytes at different fractional origins, unchanged measurement, and fractional animation behavior.

## Experimental MSDF path

MSDF remains behind the `experimental-msdf` feature and is not Lumen's production text renderer.

The original prototype scaled its generation data with glyph occurrences rather than unique glyphs. Repeated large characters duplicated jobs, outline segments, and a per-pixel job-index array; large glyph area multiplied by text count could exhaust the pixel budget or allocate very large buffers. The fixed atlas also charged MSDF range padding twice, and the outline-job cache had no bound across animated sizes/configurations. A small still frame could therefore look correct while a realistic sequence fell back, lost glyphs, or grew memory.

The revised preparation path:

- prepares each unique MSDF or raster glyph once per frame and reuses its atlas entry for repeated instances;
- removes the occurrence-sized per-pixel job map, with compute work resolving compact job pixel ranges;
- selects signed true distance independently per color channel, using orthogonality to resolve shared-corner ties before converting the selected edge to pseudo-distance;
- stores an additional true-distance channel and applies nonzero-winding sign correction using half-open flattened curve crossings, avoiding whole-row inversions at curve extrema and compound contours;
- uses a single-sample derivative-based coverage calculation in the draw shader, reducing aliasing without supersampling or extra atlas reads;
- reserves only filtering isolation around an MSDF field instead of charging its distance range twice;
- keys outline jobs by glyph and generation configuration;
- bounds the LRU-style CPU outline cache to 256 entries and 65,536 segments;
- enforces explicit glyph, segment, MSDF-pixel, and atlas limits, falling back to alpha rasterization when MSDF generation is unsuitable;
- preserves color-raster fallback for glyphs that are not represented by the outline path.

This bounds persistent CPU cache growth and makes per-frame preparation proportional to unique glyphs plus rendered instances. The current atlas and GPU resources are still rebuilt for each render preparation, however, so this is a sound experimental baseline rather than the final ownership model.

### Benchmark snapshot

These release-mode results were collected from the text benchmark's 20-frame scenario matrix with a 4096² atlas for the large scenarios. Times are mean CPU layout plus atlas/job preparation per frame; they do not include GPU compute dispatch, drawing, readback, encoding, or presentation.

| scenario                        | raster prep | GPU-MSDF prep | workload                              |
| ------------------------------- | ----------: | ------------: | ------------------------------------- |
| baseline, 64 px / 1080p         |    275.7 µs |      237.8 µs | short reference text                  |
| large, 240 px / 1080p           |     2.58 ms |       1.12 ms | repeated large glyphs                 |
| dense, 96 px / 4K               |     4.62 ms |       1.81 ms | 2,230 glyphs                          |
| subpixel motion, 160 px / 1080p |     2.71 ms |       1.07 ms | fractional frame-to-frame origin      |
| glyph churn, 128 px / 4K        |     4.50 ms |       1.38 ms | 1,712 glyphs with changing frame text |

Peak process RSS was approximately 94–96 MiB in the 4096² matrix runs. A separate 1,800-frame experimental-MSDF glyph-churn soak rendered 1,712 of 1,712 laid-out glyphs on every frame with no capacity loss. It averaged 1.40 ms CPU preparation, reached 64.2 MiB of frame working data and 8.9 MiB of used atlas data, generated at most 73 jobs / 605,759 MSDF pixels, and peaked at 79.3 MiB RSS.

Use these numbers as a regression baseline, not an end-to-end comparison. The benchmark reports laid-out versus rendered glyph ranges, working bytes, used atlas bytes, MSDF jobs/pixels, and peak RSS precisely so a fast run that silently falls back or drops glyphs is visible. See [`lumen-bench`](../bench/README.md) for scenarios, commands, and repeatability guidance.

### Remaining production work

Before MSDF can become a production path, it still needs:

- engine/composition integration with clear resource ownership;
- a persistent GPU atlas with an eviction or paging strategy instead of full-frame rebuilds;
- GPU timestamp benchmarks for generation and draw passes, plus end-to-end throughput and allocation telemetry;
- broader font coverage, including color, variable, fallback, complex-script, and malformed-outline cases;
- expanded cross-platform complex-script fixtures beyond the current Arabic, Hebrew, Devanagari, CJK, Thai, combining-mark, and mixed-bidi gallery comparisons;
- evaluated MSDF error correction and mip/filter behavior across scale extremes;
- longer encoded visual sequences with automated temporal and readback comparisons.
