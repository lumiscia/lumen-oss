---
"@lumiscia/lumen-preview": minor
"@lumiscia/lumen-bindings": minor
---

Removes preview exports nothing used. `createLumenPreviewRuntime` now returns only `LumenPreviewController`, and `LumenMediaStore`, `LumenRenderer` and `registerFontFamilyBytes` (use `registerFontFamily`) are gone from `@lumiscia/lumen-preview` and `@lumiscia/lumen-bindings`. The controller drops `togglePlay` (use `play` and `pause`), `frameRequirementsWindow`, `snapshot`, `setCompositionSyncMs` and `recordRenderTiming`. The media helpers, such as `LumenMediaBridge`, `fetchBlob` and `normalizeFps`, are imported from `@lumiscia/lumen-preview/media` instead of the package root.
