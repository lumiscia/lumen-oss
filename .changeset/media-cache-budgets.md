---
"@lumiscia/lumen-bindings": minor
---

One place to set and read the engine's media cache budgets.

`GpuCompositionRenderer::set_cache_budgets(CacheBudgets)` sets the byte budget of each cache the renderer keeps media and derived GPU data in: image textures (256 MiB), pooled textures (384 MiB), retained outputs (1 GiB), `.cube` LUTs (128 MiB), 3D models (256 MiB), environment maps (256 MiB) and `text_3d` meshes (64 MiB). The defaults are unchanged. `cache_budgets()` reads them back, and `cache_stats()` returns a `CacheStats` with the bytes each cache holds now, plus the glyph atlas. What a frame uses, and what a kept output was drawn from, is never freed for a budget. Only the image texture budget counts just idle textures. The others count each cache's total, so entries in use can push a cache past its budget and free all its idle ones. The `models` figure includes the CPU bounds kept beside each model's GPU buffers and textures, and the pool frees its spares by size bucket rather than strictly least recently used first.

**Breaking, for Rust hosts:** `set_media_texture_cache_budget(bytes)` and `DEFAULT_MEDIA_TEXTURE_CACHE_BUDGET_BYTES` are removed, along with the hidden `set_frame_graph_pool_budget` and `set_frame_graph_retained_budget`. Replace `renderer.set_media_texture_cache_budget(bytes)` with `renderer.set_cache_budgets(CacheBudgets { image_textures: bytes, ..renderer.cache_budgets() })`, and the others with the `pooled_textures` and `retained_outputs` fields in the same way. Compositions, the server and the TypeScript SDK are unaffected.
