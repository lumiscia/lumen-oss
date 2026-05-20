import { hasPreviewWorker } from "./bindings.js";
import { describeError, reportConsoleError } from "./errors.js";
import { MainPreviewDriver } from "./main-driver.js";
import { previewTimingFromCompositionJson } from "./timing.js";
import { WorkerPreviewDriver } from "./worker-driver.js";
import type {
  LumenPreviewDriverHost,
  LumenPreviewRuntimeDriver,
  LumenPreviewSessionInputs,
  LumenPreviewSessionOptions,
} from "./types.js";

const EMPTY_AUDIO_SOURCES: LumenPreviewSessionInputs["audioSources"] = [];
const EMPTY_MEDIA_SOURCES: LumenPreviewSessionInputs["mediaSources"] = [];

export class LumenPreviewSession implements LumenPreviewDriverHost {
  readonly preview: LumenPreviewSessionOptions["preview"];
  #driver: LumenPreviewRuntimeDriver | null = null;
  #inputs: LumenPreviewSessionInputs;
  #options: LumenPreviewSessionOptions;

  constructor(options: LumenPreviewSessionOptions) {
    this.preview = options.preview;
    this.#options = options;
    this.#inputs = normalizeInputs(options);
  }

  async attach(canvas: HTMLCanvasElement | null): Promise<void> {
    if (!canvas) {
      return;
    }

    const driver = this.#createDriver(canvas);
    this.#driver = driver;
    await driver.attach(canvas);
  }

  update(options: Partial<LumenPreviewSessionOptions>): void {
    this.#options = {
      ...this.#options,
      ...options,
    };
    this.#inputs = normalizeInputs(this.#options);
    this.#driver?.update(this.#inputs);
  }

  dispose(): void {
    this.#driver?.dispose();
    this.#driver = null;
  }

  attachController: LumenPreviewDriverHost["attachController"] = (controller, seek, transport) => {
    this.preview.attach(controller, seek, transport);
  };

  detachController(): void {
    this.preview.detach();
  }

  updateState(patch: Parameters<LumenPreviewDriverHost["updateState"]>[0]): void {
    this.preview.update(patch);
  }

  reportError(scope: string, error: unknown): void {
    reportConsoleError(scope, error);
    this.preview.update({ error: describeError(error) });
  }

  reportStats(stats: Parameters<LumenPreviewDriverHost["reportStats"]>[0]): void {
    this.#inputs.onStats?.(stats);
  }

  #createDriver(canvas: HTMLCanvasElement): LumenPreviewRuntimeDriver {
    if (
      hasPreviewWorker(this.#inputs.bindings) &&
      typeof canvas.transferControlToOffscreen === "function"
    ) {
      return new WorkerPreviewDriver(this, this.#inputs, this.#options, this.#inputs.bindings);
    }

    return new MainPreviewDriver(this, this.#inputs, this.#options);
  }
}

function normalizeInputs(options: LumenPreviewSessionOptions): LumenPreviewSessionInputs {
  const timing = previewTimingFromCompositionJson(options.compositionJson);
  return {
    audioSources: options.audioSources ?? EMPTY_AUDIO_SOURCES,
    audioTimeline: options.audioTimeline ?? null,
    bindings: options.bindings ?? missingBindings(),
    compositionJson: options.compositionJson ?? null,
    fps: timing.fps,
    targetFrameDurationMs: timing.targetFrameDurationMs,
    logLevel: options.logLevel ?? "off",
    mediaSources: options.mediaSources ?? EMPTY_MEDIA_SOURCES,
    onStats: options.onStats ?? null,
  };
}

function missingBindings(): never {
  throw new Error("LumenPreviewSession requires `bindings`");
}
