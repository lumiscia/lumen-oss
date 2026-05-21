export interface LumenPreviewStats {
  frame: number;
  timelineFps: number;
  targetFrameDurationMs: number;
  renderMs: number;
  actualFps: number;
}

export type LumenPreviewStatsCallback = (stats: LumenPreviewStats) => void;

export const EMPTY_PREVIEW_STATS: LumenPreviewStats = {
  frame: 0,
  timelineFps: 0,
  targetFrameDurationMs: 0,
  renderMs: 0,
  actualFps: 0,
};

const FPS_WINDOW_MS = 1_000;

export class PlaybackFpsMeter {
  #fps = 0;
  #samples: Array<{ frame: number; time: number }> = [];

  sample(frame: number, isPlaying: boolean, now: number = performance.now()): number {
    if (!isPlaying) {
      return this.#fps;
    }

    const previous = this.#samples.at(-1);
    if (!previous || frame !== previous.frame) {
      this.#samples.push({ frame, time: now });
    }

    const windowStart = now - FPS_WINDOW_MS;
    while (this.#samples.length > 1) {
      const oldest = this.#samples[0];
      if (!oldest || oldest.time >= windowStart) {
        break;
      }
      this.#samples.shift();
    }

    const first = this.#samples[0];
    const last = this.#samples.at(-1);
    if (!first || !last || first === last) {
      return this.#fps;
    }

    const frameDelta = last.frame - first.frame;
    const timeDelta = last.time - first.time;
    if (frameDelta > 0 && timeDelta > 0) {
      this.#fps = (frameDelta * 1_000) / timeDelta;
    }

    return this.#fps;
  }

  reset(): void {
    this.#fps = 0;
    this.#samples = [];
  }
}
