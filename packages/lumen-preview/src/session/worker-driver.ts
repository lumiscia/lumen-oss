import { LumenAudioEngine, audioTimelineFromCompositionJson } from "../audio-engine.js";
import type { LumenPreviewWorkerCommand, LumenPreviewWorkerEvent } from "../worker-host.js";
import { createWorkerControllerProxy } from "./controller-proxy.js";
import type {
  LumenPreviewDriverHost,
  LumenPreviewRuntimeDriver,
  LumenPreviewSessionInputs,
  LumenPreviewSessionOptions,
  LumenWorkerBindings,
} from "./types.js";

export class WorkerPreviewDriver implements LumenPreviewRuntimeDriver {
  #audio: LumenAudioEngine;
  #host: LumenPreviewDriverHost;
  #inputs: LumenPreviewSessionInputs;
  #offscreenTransferred = false;
  #worker: Worker | null = null;
  readonly #bindings: LumenWorkerBindings;

  constructor(
    host: LumenPreviewDriverHost,
    inputs: LumenPreviewSessionInputs,
    options: LumenPreviewSessionOptions,
    bindings: LumenWorkerBindings,
  ) {
    this.#host = host;
    this.#inputs = inputs;
    this.#bindings = bindings;
    this.#audio = new LumenAudioEngine(options.audio);
  }

  async attach(canvas: HTMLCanvasElement): Promise<void> {
    if (this.#offscreenTransferred || typeof canvas.transferControlToOffscreen !== "function") {
      throw new Error("OffscreenCanvas transfer is not available for this canvas");
    }

    const worker = new Worker(String(this.#bindings.previewWorkerUrl()), { type: "module" });
    const offscreen = canvas.transferControlToOffscreen();
    this.#offscreenTransferred = true;
    this.#worker = worker;

    worker.onmessage = (event: MessageEvent<LumenPreviewWorkerEvent>) => {
      if (event.data.type === "state") {
        this.#host.updateState(event.data.patch);
        return;
      }
      if (event.data.type === "stats") {
        this.#host.reportStats(event.data.stats);
        return;
      }
      this.#host.reportError(event.data.scope, event.data.message);
    };

    this.#post(
      {
        type: "initialize",
        canvas: offscreen,
        compositionJson: this.#inputs.compositionJson,
        logLevel: this.#inputs.logLevel,
        mediaSources: this.#inputs.mediaSources,
      },
      [offscreen],
    );

    this.#syncAudio();
    this.#host.attachController(
      createWorkerControllerProxy(this.#host.preview, () => this.#inputs),
      (frame) => this.#post({ type: "seek", frame }),
      {
        pause: () => {
          this.#audio.pause();
          this.#post({ type: "pause" });
        },
        play: () => {
          const frame = this.#host.preview.getSnapshot().frame;
          const fromMs = (frame / Math.max(this.#inputs.fps, 1)) * 1_000;
          this.#audio.play(fromMs);
          this.#post({ type: "play", fromMs });
        },
        seek: (frame) => {
          this.#audio.seekMs((frame / Math.max(this.#inputs.fps, 1)) * 1_000);
          this.#post({ type: "seek", frame });
        },
      },
    );
  }

  update(inputs: LumenPreviewSessionInputs): void {
    const previous = this.#inputs;
    this.#inputs = inputs;
    this.#post({ type: "set-log-level", logLevel: inputs.logLevel });
    this.#syncAudio();

    if (
      previous.compositionJson !== inputs.compositionJson ||
      previous.mediaSources !== inputs.mediaSources
    ) {
      this.#post({
        type: "set-composition",
        compositionJson: inputs.compositionJson,
        mediaSources: inputs.mediaSources,
      });
    }
  }

  dispose(): void {
    this.#post({ type: "dispose" });
    this.#worker?.terminate();
    this.#worker = null;
    this.#audio.dispose();
    this.#host.detachController();
  }

  #syncAudio(): void {
    const timeline =
      this.#inputs.audioTimeline ?? audioTimelineFromCompositionJson(this.#inputs.compositionJson);
    this.#audio.setAudioTimeline(timeline);
    void this.#audio.syncAudioSources(this.#inputs.audioSources).catch((error: unknown) => {
      this.#host.reportError("sync audio sources", error);
    });
  }

  #post(message: LumenPreviewWorkerCommand, transfer: Transferable[] = []): void {
    this.#worker?.postMessage(message, transfer);
  }
}
