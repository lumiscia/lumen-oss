import { useEffect, useRef } from "react";

import type { CSSProperties } from "react";
import * as defaults from "@lumiscia/lumen-bindings/urls";
import { mountPreview, type LumenPreviewSession } from "@lumiscia/lumen-preview";
import type { LumenPreviewSessionInputs } from "@lumiscia/lumen-preview";

import type { LumenPreviewContext } from "./preview.js";

export interface PlayerProps extends LumenPreviewSessionInputs {
  preview: LumenPreviewContext;
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
  const wasm = String(wasmUrl ?? defaults.wasmUrl);
  const worker = String(workerUrl ?? defaults.workerUrl);

  useEffect(() => {
    const session = mountPreview(hostRef.current as HTMLDivElement, {
      ...inputs,
      preview,
      wasmUrl: wasm,
      workerUrl: worker,
    });
    sessionRef.current = session;
    return () => session.dispose();
  }, [preview, wasm, worker]);

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
