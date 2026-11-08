const PREVIEW_FALLBACK =
  "If it isn't served there, pass `workerUrl` and `wasmUrl` pointing at `@lumiscia/lumen-bindings/worker` and `@lumiscia/lumen-bindings/wasm`, for example with Vite's `?url` imports.";

/** How to fix a Lumen file that didn't load from `url`, with `fallback` when it isn't Vite's. */
export function assetHint(url: string, fallback = PREVIEW_FALLBACK): string {
  if (URL.canParse(url) && new URL(url).pathname.includes("/.vite/deps/")) {
    return 'Vite 6 and 7 move it there when they pre-bundle dependencies in dev; add `optimizeDeps: { exclude: ["@lumiscia/lumen-bindings", "@lumiscia/lumen-preview"] }` to the Vite config.';
  }
  return fallback;
}
