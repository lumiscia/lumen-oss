const FALLBACK =
  "If it isn't served there, copy the files the `runtime` manifest names into one directory and pass `runtimeUrl`, or pass a `runtime` whose URLs say where each file is served.";

/** How to fix a Lumen runtime file that didn't load from `url`. */
export function assetHint(url: string): string {
  if (pathOf(url).includes("/.vite/deps/")) {
    return 'Vite 6 and 7 move it there when they pre-bundle dependencies in dev; add `optimizeDeps: { exclude: ["@lumiscia/lumen-bindings"] }` to the Vite config.';
  }
  return FALLBACK;
}

function pathOf(url: string): string {
  try {
    return new URL(url).pathname;
  } catch {
    return "";
  }
}
