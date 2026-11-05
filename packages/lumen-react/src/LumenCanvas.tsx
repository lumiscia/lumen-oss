import { useEffect, useRef } from "react";

import type { CSSProperties } from "react";
import { mountPreview, type LumenPreviewSession } from "@lumiscia/lumen-preview";
import type { LumenPreviewSessionInputs, LumenWorkerBindings } from "@lumiscia/lumen-preview";

import type { LumenPreviewContext } from "./preview.js";

export interface LumenCanvasProps extends LumenPreviewSessionInputs {
  preview: LumenPreviewContext;
  bindings: LumenWorkerBindings;
  /** Applied to the element that holds the canvas, which fills it and keeps its aspect ratio. */
  className?: string;
  style?: CSSProperties;
}

export function LumenCanvas({
  preview,
  bindings,
  composition,
  fonts,
  lookaheadCount,
  logLevel,
  media,
  onStats,
  className,
  style,
}: LumenCanvasProps) {
  const hostRef = useRef<HTMLDivElement | null>(null);
  const sessionRef = useRef<LumenPreviewSession | null>(null);
  const inputs = { composition, fonts, lookaheadCount, logLevel, media, onStats };

  useEffect(() => {
    const session = mountPreview(hostRef.current as HTMLDivElement, {
      ...inputs,
      bindings,
      preview,
    });
    sessionRef.current = session;
    return () => session.dispose();
  }, [preview, bindings]);

  useEffect(() => {
    sessionRef.current?.update(inputs);
  }, [composition, fonts, lookaheadCount, logLevel, media, onStats]);

  return <div ref={hostRef} className={className} style={style} />;
}
