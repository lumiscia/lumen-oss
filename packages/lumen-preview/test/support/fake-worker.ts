import type { LumenPreviewWorkerCommand, LumenPreviewWorkerEvent } from "../../src/worker-host.js";

/** A stand-in for the browser's `Worker` that records what the session posts and lets a test reply. */
export class FakeWorker {
  static instances: FakeWorker[] = [];
  onerror: ((event: { message: string }) => void) | null = null;
  onmessage: ((event: { data: LumenPreviewWorkerEvent }) => void) | null = null;
  onmessageerror: (() => void) | null = null;
  posted: LumenPreviewWorkerCommand[] = [];
  terminated = false;

  constructor(readonly url: string) {
    FakeWorker.instances.push(this);
  }
  postMessage(message: LumenPreviewWorkerCommand) {
    this.posted.push(message);
  }
  terminate() {
    this.terminated = true;
  }
  emit(data: LumenPreviewWorkerEvent) {
    this.onmessage?.({ data });
  }
  ofType<TType extends LumenPreviewWorkerCommand["type"]>(type: TType) {
    return this.posted.filter(
      (message): message is Extract<LumenPreviewWorkerCommand, { type: TType }> =>
        message.type === type,
    );
  }
}
