import type {
  AudioEngineTimeline,
  AudioSourceRegistration,
  LumenAudioEngineOptions,
} from "../audio-engine.js";
import type {
  LumenBindings,
  LumenLogLevel,
  LumenPreviewBindingSource,
  LumenPreviewController,
} from "../index.js";
import type { MediaRegistration } from "../media/index.js";
import type { LumenPreviewContext, LumenPreviewPatch } from "../preview.js";
import type { LumenPreviewStats, LumenPreviewStatsCallback } from "./stats.js";

export interface LumenPreviewSessionInputs {
  audioSources: readonly AudioSourceRegistration[];
  audioTimeline: AudioEngineTimeline | null;
  bindings: LumenPreviewBindingSource;
  compositionJson: string | null;
  fps: number;
  targetFrameDurationMs: number;
  logLevel: LumenLogLevel;
  mediaSources: readonly MediaRegistration[];
  onStats: LumenPreviewStatsCallback | null;
}

export interface LumenPreviewSessionOptions extends Partial<
  Omit<LumenPreviewSessionInputs, "fps" | "targetFrameDurationMs">
> {
  audio?: LumenAudioEngineOptions;
  preview: LumenPreviewContext;
}

export interface LumenPreviewRuntimeDriver {
  attach(canvas: HTMLCanvasElement): Promise<void>;
  update(inputs: LumenPreviewSessionInputs): void;
  dispose(): void;
}

export interface LumenPreviewDriverHost {
  readonly preview: LumenPreviewContext;
  attachController(
    controller: LumenPreviewController,
    seek: (frame: number) => void,
    transport: {
      pause: () => void;
      play: () => void;
      seek: (frame: number) => void;
    },
  ): void;
  detachController(): void;
  reportError(scope: string, error: unknown): void;
  reportStats(stats: LumenPreviewStats): void;
  updateState(patch: LumenPreviewPatch): void;
}

export type LumenWorkerBindings = LumenBindings & {
  previewWorkerUrl: () => string | URL;
};
