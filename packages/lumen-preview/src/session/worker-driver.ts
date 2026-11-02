import { LumenAudioEngine } from "../audio-engine.js";
import type { LumenPreviewWorkerCommand, LumenPreviewWorkerEvent } from "../worker-host.js";
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
  #loadId = 0;
  #offscreenTransferred = false;
  #timelineFps = 0;
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
    this.#audio = new LumenAudioEngine(options.audio, "elapsed");
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
      if (event.data.type === "audio-timeline") {
        if (event.data.loadId === this.#loadId) {
          this.#audio.loadAudioTimeline(event.data.timelineJson);
          this.#syncAudioSources();
        }
        return;
      }
      if (event.data.type === "state") {
        this.#host.updateState(event.data.patch);
        return;
      }
      if (event.data.type === "stats") {
        this.#timelineFps = event.data.stats.timelineFps;
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
        loadId: this.#loadId,
        lookaheadCount: this.#inputs.lookaheadCount,
        logLevel: this.#inputs.logLevel,
        mediaSources: this.#inputs.mediaSources,
      },
      [offscreen],
    );

    this.#host.attachController(null, (frame) => this.#post({ type: "seek", frame }), {
      pause: () => {
        this.#audio.pause();
        this.#post({ type: "pause" });
      },
      play: () => {
        const frame = this.#host.preview.getSnapshot().frame;
        const fromMs = this.#frameToMs(frame);
        this.#audio.play(fromMs);
        this.#post({ type: "play", fromMs });
      },
      seek: (frame) => {
        this.#audio.seekMs(this.#frameToMs(frame));
        this.#post({ type: "seek", frame });
      },
    });
  }

  update(inputs: LumenPreviewSessionInputs): void {
    const previous = this.#inputs;
    this.#inputs = inputs;
    this.#post({ type: "set-log-level", logLevel: inputs.logLevel });
    this.#post({ type: "set-lookahead-count", lookaheadCount: inputs.lookaheadCount });
    if (previous.audioSources !== inputs.audioSources) {
      this.#syncAudioSources();
    }

    if (
      previous.compositionJson !== inputs.compositionJson ||
      previous.mediaSources !== inputs.mediaSources
    ) {
      // The video is frozen until the worker answers with the new timeline.
      this.#audio.loadAudioTimeline(null);
      this.#post({
        type: "set-composition",
        compositionJson: inputs.compositionJson,
        loadId: ++this.#loadId,
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

  #syncAudioSources(): void {
    void this.#audio.syncAudioSources(this.#inputs.audioSources).catch((error: unknown) => {
      this.#host.reportError("sync audio sources", error);
    });
  }

  #post(message: LumenPreviewWorkerCommand, transfer: Transferable[] = []): void {
    this.#worker?.postMessage(message, transfer);
  }

  #frameToMs(frame: number): number {
    return (frame / Math.max(this.#timelineFps, 1)) * 1_000;
  }
}
