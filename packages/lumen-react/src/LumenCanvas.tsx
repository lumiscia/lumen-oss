import { useEffect, useRef } from "react";

import type { CSSProperties } from "react";
import { LumenPreviewSession } from "@lumiscia/lumen-preview";
import type {
  AudioSourceRegistration,
  LumenLogLevel,
  LumenPreviewBindingSource,
  LumenPreviewStatsCallback,
  MediaRegistration,
} from "@lumiscia/lumen-preview";

import type { LumenPreviewContext } from "./preview.ts";

const EMPTY_AUDIO_SOURCES: AudioSourceRegistration[] = [];
const EMPTY_MEDIA_SOURCES: MediaRegistration[] = [];

export interface LumenCanvasProps {
  preview: LumenPreviewContext;
  bindings: LumenPreviewBindingSource;
  audioSources?: AudioSourceRegistration[];
  compositionJson?: string | null;
  mediaSources?: MediaRegistration[];
  logLevel?: LumenLogLevel;
  onStats?: LumenPreviewStatsCallback;
  className?: string;
  style?: CSSProperties;
}

export function LumenCanvas({
  preview,
  bindings,
  audioSources = EMPTY_AUDIO_SOURCES,
  compositionJson = null,
  mediaSources = EMPTY_MEDIA_SOURCES,
  logLevel = "off",
  onStats,
  className,
  style,
}: LumenCanvasProps) {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const sessionRef = useRef<LumenPreviewSession | null>(null);

  useEffect(() => {
    const session = new LumenPreviewSession({
      preview,
      bindings,
      audioSources,
      compositionJson,
      mediaSources,
      logLevel,
      onStats: onStats ?? null,
    });
    sessionRef.current = session;
    void session.attach(canvasRef.current).catch((error: unknown) => {
      preview.update({
        error: error instanceof Error ? (error.stack ?? error.message) : String(error),
      });
    });

    return () => {
      session.dispose();
      sessionRef.current = null;
    };
  }, [preview, bindings]);

  useEffect(() => {
    sessionRef.current?.update({
      audioSources,
      compositionJson,
      mediaSources,
      logLevel,
      onStats: onStats ?? null,
    });
  }, [audioSources, compositionJson, mediaSources, logLevel, onStats]);

  const snapshot = preview.getSnapshot();

  return (
    <canvas
      ref={canvasRef}
      width={snapshot.width || 1}
      height={snapshot.height || 1}
      className={className}
      style={style}
    />
  );
}
