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

export interface LumenPreviewSessionInputs {
  audioSources: readonly AudioSourceRegistration[];
  audioTimeline: AudioEngineTimeline | null;
  bindings: LumenPreviewBindingSource;
  compositionJson: string | null;
  fps: number;
  frameDurationMs: number;
  logLevel: LumenLogLevel;
  mediaSources: readonly MediaRegistration[];
}

export interface LumenPreviewSessionOptions extends Partial<
  Omit<LumenPreviewSessionInputs, "fps" | "frameDurationMs">
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
  updateState(patch: LumenPreviewPatch): void;
}

export type LumenWorkerBindings = LumenBindings & {
  previewWorkerUrl: () => string | URL;
};
