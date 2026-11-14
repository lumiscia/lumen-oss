import type { MediaSourceInput } from "../media/index.js";
import type { PreviewMediaEntry } from "../media/dependencies.js";
import type { LumenPreviewWorkerCommand } from "../worker-host.js";
import type { Load } from "./load.js";
import { Transport, type TransportHost } from "./transport.js";

export type PreviewWorkerOptions = Omit<
  Extract<LumenPreviewWorkerCommand, { type: "set-options" }>,
  "type"
>;

/**
 * Everything one attach to a host element owns: the worker, the canvas, the playback and the
 * load state that go with them. Detaching disposes it and drops it, so nothing of it survives
 * into the next attach.
 */
export class Attachment {
  readonly startup = new AbortController();
  readonly transport: Transport;
  canvas: HTMLCanvasElement | null = null;
  // The newest load of the inputs, or null before the first.
  load: Load | null = null;
  // The frame onTimeUpdate last reported, so a frame already shown isn't reported again.
  reportedFrame: number | null = null;
  // Why the inputs last failed to resolve. The same failure isn't reported again, so a callback that
  // re-renders with the same inputs can't loop.
  resolveFailure: string | null = null;
  sentFonts: ReadonlyMap<string, MediaSourceInput> = new Map();
  sentMedia: ReadonlyMap<string, PreviewMediaEntry> = new Map();
  worker: Worker | null = null;

  constructor(
    public workerOptions: PreviewWorkerOptions,
    report: TransportHost["report"],
  ) {
    this.transport = new Transport({
      isLoading: () => this.load?.isLoading ?? false,
      mediaPending: () => this.load?.mediaPending ?? false,
      post: (message) => this.post(message),
      report,
    });
  }

  post(message: LumenPreviewWorkerCommand, transfer: Transferable[] = []): void {
    this.worker?.postMessage(message, transfer);
  }

  dispose(): void {
    this.startup.abort();
    this.transport.dispose();
    this.worker?.terminate();
    this.canvas?.remove();
  }
}
