import { useEffect, useRef } from "react";

import type { CSSProperties } from "react";
import * as defaults from "@lumiscia/lumen-bindings/urls";
import { mountPreview, type LumenPreviewSession } from "@lumiscia/lumen-preview";
import type { LumenPreviewSessionInputs } from "@lumiscia/lumen-preview";

import type { LumenPreviewContext } from "./preview.js";

export interface PlayerProps extends LumenPreviewSessionInputs {
  preview: LumenPreviewContext;
  /**
   * A directory serving the preview's runtime files: `worker.js` and `lumen_wasm_bg.wasm` from
   * `@lumiscia/lumen-bindings`, and `audio-worker.js` and `audio-worklet.js` from
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

export function Player({
  preview,
  runtimeUrl,
  wasmUrl,
  workerUrl,
  composition,
  fonts,
  loop,
  lookaheadCount,
  logLevel,
  media,
  muted,
  onEnded,
  onError,
  onStats,
  onTimeUpdate,
  range,
  rate,
  volume,
  className,
  style,
}: PlayerProps) {
  const hostRef = useRef<HTMLDivElement | null>(null);
  const sessionRef = useRef<LumenPreviewSession | null>(null);
  const inputs = {
    composition,
    fonts,
    loop,
    lookaheadCount,
    logLevel,
    media,
    muted,
    onEnded,
    onError,
    onStats,
    onTimeUpdate,
    range,
    rate,
    volume,
  };
  const runtime = runtimeUrl === undefined ? undefined : String(runtimeUrl);
  const wasm = runtimeAssetUrl(wasmUrl, runtime, defaults.wasmUrl);
  const worker = runtimeAssetUrl(workerUrl, runtime, defaults.workerUrl);

  useEffect(() => {
    const session = mountPreview(hostRef.current as HTMLDivElement, {
      ...inputs,
      preview,
      runtimeUrl: runtime,
      wasmUrl: wasm,
      workerUrl: worker,
    });
    sessionRef.current = session;
    return () => session.dispose();
  }, [preview, runtime, wasm, worker]);

  useEffect(() => {
    sessionRef.current?.update(inputs);
  }, [
    composition,
    fonts,
    loop,
    lookaheadCount,
    logLevel,
    media,
    muted,
    onEnded,
    onError,
    onStats,
    onTimeUpdate,
    // A range written inline in JSX is a new object on every render.
    range?.in,
    range?.out,
    rate,
    volume,
  ]);

  return <div ref={hostRef} className={className} style={style} />;
}

/** The URL passed, else none when `runtimeUrl` gives it, else the package's own copy. */
function runtimeAssetUrl(
  url: string | URL | undefined,
  runtimeUrl: string | undefined,
  fallback: URL,
): string | undefined {
  if (url !== undefined) {
    return String(url);
  }
  return runtimeUrl === undefined ? String(fallback) : undefined;
}
