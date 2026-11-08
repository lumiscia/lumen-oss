/** How to fix a Lumen worker or wasm that didn't load from `url`. */
export function assetHint(url: string): string {
  if (url.includes("/.vite/deps/")) {
    return 'Vite 6 and 7 move it there when they pre-bundle dependencies in dev; add `optimizeDeps: { exclude: ["@lumiscia/lumen-bindings", "@lumiscia/lumen-preview"] }` to the Vite config.';
  }
  return "If it isn't served there, pass `workerUrl` and `wasmUrl` pointing at `@lumiscia/lumen-bindings/worker` and `@lumiscia/lumen-bindings/wasm`, for example with Vite's `?url` imports.";
}
