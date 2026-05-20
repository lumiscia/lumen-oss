export class LumenRenderQueue {
  #inFlight = false;
  #queued: (() => Promise<void>) | null = null;

  enqueue(operation: () => Promise<void>): void {
    this.#queued = operation;
    if (this.#inFlight) {
      return;
    }

    void this.#drain();
  }

  clear(): void {
    this.#queued = null;
    this.#inFlight = false;
  }

  async #drain(): Promise<void> {
    this.#inFlight = true;

    try {
      while (this.#queued) {
        const next = this.#queued;
        this.#queued = null;
        await next();
      }
    } finally {
      this.#inFlight = false;
      if (this.#queued) {
        void this.#drain();
      }
    }
  }
}
