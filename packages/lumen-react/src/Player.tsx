import { useEffect, useRef } from "react";

import type { CSSProperties } from "react";
import { mountPreview, runtimeFileUrl, type LumenPreviewSession } from "@lumiscia/lumen-preview";
import type { LumenPreviewSessionInputs } from "@lumiscia/lumen-preview";

import type { LumenPreviewContext } from "./preview.js";

export interface PlayerProps extends LumenPreviewSessionInputs {
  preview: LumenPreviewContext;
  /**
   * A directory serving the preview's runtime files: `worker.js` and `lumen_wasm_bg.wasm` (or
   * `worker-2d.js` and `lumen_wasm_2d_bg.wasm`, for the `Player` from `@lumiscia/lumen-react/2d`)
   * from `@lumiscia/lumen-bindings`, and `audio-worker.js` and `audio-worklet.js` from
   * `@lumiscia/lumen-preview`. When set, every runtime URL not passed on its own is that file in
   * it; otherwise each is found next to its package.
   */
  runtimeUrl?: string | URL | undefined;
  /** Where the preview worker's wasm is served. Defaults to the copy in `@lumiscia/lumen-bindings`. */
  wasmUrl?: string | URL | undefined;
  /** Where the preview worker is served. Defaults to the copy in `@lumiscia/lumen-bindings`. */
  workerUrl?: string | URL | undefined;
  /** Applied to the element that holds the canvas, which fills it and keeps its aspect ratio. */
  className?: string;
  style?: CSSProperties;
}

/** The wasm and worker a `Player` falls back to, and their file names in `runtimeUrl`. */
export interface PlayerDefaults {
  wasmUrl: URL;
  wasmFile: string;
  workerUrl: URL;
  workerFile: string;
}

/** A `Player` whose wasm defaults to `defaults`, so an entry point names only the wasm it uses. */
export function createPlayer(defaults: PlayerDefaults) {
  return function Player(props: PlayerProps) {
    return <PreviewPlayer {...props} defaults={defaults} />;
  };
}

function PreviewPlayer({
  defaults,
  preview,
  runtimeUrl,
  wasmUrl,
  workerUrl,
  className,
  style,
  ...inputs
}: PlayerProps & { defaults: PlayerDefaults }) {
  const hostRef = useRef<HTMLDivElement | null>(null);
  const sessionRef = useRef<LumenPreviewSession | null>(null);
  const runtime = runtimeUrl === undefined ? undefined : String(runtimeUrl);
  const wasm = wasmUrl === undefined ? undefined : String(wasmUrl);
  const worker = workerUrl === undefined ? undefined : String(workerUrl);

  useEffect(() => {
    // Resolved here rather than while rendering, which can run on a server without `document`.
    const session = mountPreview(hostRef.current as HTMLDivElement, {
      ...inputs,
      preview,
      runtimeUrl: runtime,
      wasmUrl: runtimeAssetUrl(wasm, runtime, defaults.wasmUrl, defaults.wasmFile),
      workerUrl: runtimeAssetUrl(worker, runtime, defaults.workerUrl, defaults.workerFile),
    });
    sessionRef.current = session;
    return () => session.dispose();
  }, [preview, runtime, wasm, worker, defaults]);

  // The session ignores inputs that haven't changed, so every render can pass them all on.
  useEffect(() => {
    sessionRef.current?.setInputs(inputs);
  });

  return <div ref={hostRef} className={className} style={style} />;
}

/** The URL passed, else `file` in `runtimeUrl`, else the package's own copy. */
function runtimeAssetUrl(
  url: string | undefined,
  runtimeUrl: string | undefined,
  fallback: URL,
  file: string,
): string {
  if (url !== undefined) {
    return url;
  }
  return runtimeUrl === undefined ? String(fallback) : runtimeFileUrl(runtimeUrl, file);
}
