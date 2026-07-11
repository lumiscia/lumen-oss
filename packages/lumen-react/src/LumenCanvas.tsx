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

import { useLumenPreview, type LumenPreviewContext } from "./preview.js";

const EMPTY_AUDIO_SOURCES: AudioSourceRegistration[] = [];
const EMPTY_MEDIA_SOURCES: MediaRegistration[] = [];

export interface LumenCanvasProps {
  preview: LumenPreviewContext;
  bindings: LumenPreviewBindingSource;
  audioSources?: AudioSourceRegistration[];
  compositionJson?: string | null;
  mediaSources?: MediaRegistration[];
  lookaheadCount?: number;
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
  lookaheadCount,
  logLevel = "off",
  onStats,
  className,
  style,
}: LumenCanvasProps) {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const sessionRef = useRef<LumenPreviewSession | null>(null);
  const snapshot = useLumenPreview(preview);

  useEffect(() => {
    let active = true;
    const session = new LumenPreviewSession({
      preview,
      bindings,
      audioSources,
      compositionJson,
      mediaSources,
      ...(lookaheadCount === undefined ? {} : { lookaheadCount }),
      logLevel,
      onStats: onStats ?? null,
    });
    sessionRef.current = session;
    void session.attach(canvasRef.current).catch((error: unknown) => {
      if (!active) {
        return;
      }
      preview.update({
        error: error instanceof Error ? (error.stack ?? error.message) : String(error),
      });
    });

    return () => {
      active = false;
      session.dispose();
      if (sessionRef.current === session) {
        sessionRef.current = null;
      }
    };
  }, [preview, bindings]);

  useEffect(() => {
    sessionRef.current?.update({
      audioSources,
      compositionJson,
      mediaSources,
      ...(lookaheadCount === undefined ? {} : { lookaheadCount }),
      logLevel,
      onStats: onStats ?? null,
    });
  }, [audioSources, compositionJson, mediaSources, lookaheadCount, logLevel, onStats]);

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
