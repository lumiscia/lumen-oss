type FrameRequirementSource = {
  durationFrames: () => number;
  load: (requirementsJson: string) => Promise<void>;
  requirements: (frame: number) => string;
};

export class FrameRequirementLoader {
  #generation = 0;
  #lookaheadCount = 8;
  readonly #pending = new Map<number, Promise<void>>();
  readonly #source: FrameRequirementSource;

  constructor(source: FrameRequirementSource) {
    this.#source = source;
  }

  reset(): void {
    this.#generation += 1;
    this.#pending.clear();
  }

  setLookaheadCount(lookaheadCount: number): void {
    this.#lookaheadCount = Math.max(0, Math.floor(lookaheadCount));
    this.reset();
  }

  async loadFrame(frame: number): Promise<void> {
    await this.#load(frame);
  }

  prefetchWindow(frame: number): void {
    if (this.#source.durationFrames() <= 0) {
      return;
    }

    void this.#prefetchFrames(frame).catch(() => {
      // The foreground render path reports persistent media errors.
    });
  }

  async #prefetchFrames(frame: number): Promise<void> {
    const generation = this.#generation;
    for (let offset = 0; offset < this.#lookaheadCount; offset += 1) {
      if (generation !== this.#generation) {
        return;
      }
      await this.loadFrame(frame + offset);
    }
  }

  async #load(frame: number): Promise<void> {
    const normalizedFrame = this.#normalizeFrame(frame);
    const existing = this.#pending.get(normalizedFrame);
    if (existing) {
      await existing;
      return;
    }

    const generation = this.#generation;
    const pending = (async () => {
      const requirementsJson = this.#source.requirements(normalizedFrame);
      if (generation === this.#generation) {
        await this.#source.load(requirementsJson);
      }
    })().finally(() => {
      if (this.#pending.get(normalizedFrame) === pending) {
        this.#pending.delete(normalizedFrame);
      }
    });
    this.#pending.set(normalizedFrame, pending);

    try {
      await pending;
    } catch (error) {
      if (generation === this.#generation) {
        throw error;
      }
    }
  }

  #normalizeFrame(frame: number): number {
    const totalFrames = this.#source.durationFrames();
    if (totalFrames <= 0) {
      return frame;
    }

    return ((frame % totalFrames) + totalFrames) % totalFrames;
  }
}
