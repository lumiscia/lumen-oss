import type { LumenRuntime } from "@lumiscia/lumen-preview";

/**
 * The preview's runtime files for the 2D-only wasm, which reports 3D nodes as `node.unsupported`.
 * Its wasm runs only in its own worker.
 */
export declare const runtime2d: LumenRuntime;
