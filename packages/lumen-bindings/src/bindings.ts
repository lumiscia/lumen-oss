import type { LumenBindings } from "@lumiscia/lumen-preview";

import initPreview, * as previewBindings from "../wasm/lumen_wasm.js";

let ready: Promise<void> | null = null;

/**
 * A page has one wasm instance, and installing another invalidates every controller made from the
 * first. wasm-bindgen's async init checks for one only before it starts, so this compiles first and
 * installs with `initSync`, which keeps whichever instance got there first, including a caller's.
 * The one exception is the async fallback below.
 */
function initialize(wasmUrl: string | URL): Promise<void> {
  ready ??= install(wasmUrl).catch((error: unknown) => {
    ready = null;
    throw error;
  });
  return ready;
}

async function install(wasmUrl: string | URL): Promise<void> {
  const response = await fetch(wasmUrl);
  if (!response.ok) {
    throw new Error(`could not fetch the Lumen wasm from ${wasmUrl}: ${response.status}`);
  }
  const module = await WebAssembly.compile(await response.arrayBuffer());
  try {
    previewBindings.initSync({ module });
  } catch (error) {
    if (!(error instanceof RangeError)) {
      throw error;
    }
    // Chromium won't instantiate over 8 MB synchronously on the main thread, so this module can only
    // install asynchronously. A caller's `initSync` of the same module throws the same way, but one
    // of a different module under 8 MB would succeed, and this would then replace its instance.
    await initPreview({ module_or_path: module });
  }
}

export function createBindings(wasmUrl: string | URL): LumenBindings {
  return {
    preview: async () => {
      await initialize(wasmUrl);
      return previewBindings;
    },
  };
}
