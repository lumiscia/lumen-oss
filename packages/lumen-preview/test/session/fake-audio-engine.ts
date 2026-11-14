import { vi } from "vitest";

type FakeRange = { endMs: number; loop: boolean; startMs: number };

export const audio = {
  clockRunning: false,
  instances: [] as {
    loadAudioTimeline: ReturnType<typeof vi.fn>;
    loadSource: ReturnType<typeof vi.fn>;
    options: { onClockChange: () => void; workerUrl?: unknown; workletUrl?: unknown };
    pause: ReturnType<typeof vi.fn>;
    play: ReturnType<typeof vi.fn>;
    removeSource: ReturnType<typeof vi.fn>;
    seekMs: ReturnType<typeof vi.fn>;
    setRange: ReturnType<typeof vi.fn>;
    setRate: ReturnType<typeof vi.fn>;
    setVolume: ReturnType<typeof vi.fn>;
  }[],
  playing: false,
  positionMs: 0,
};

// Follows the engine: an identical timeline is left playing, only segments play, and the position
// wraps round the range, or round the whole timeline when it has none.
export class FakeAudioEngine {
  range: FakeRange | null = null;
  timeline: { durationFrames: number; fps: number; segments: unknown[] } | null = null;
  timelineJson: string | null = null;
  loadAudioTimeline = vi.fn(
    (json: string | null, length: { durationFrames: number; fps: number }) => {
      const key = json === null ? null : JSON.stringify([json, length]);
      if (key === this.timelineJson) {
        return false;
      }
      this.timelineJson = key;
      this.timeline = json === null ? null : { ...JSON.parse(json), ...length };
      audio.playing = false;
      audio.positionMs = 0;
      return true;
    },
  );
  play = vi.fn((ms?: number) => {
    audio.positionMs = this.wrap(ms ?? audio.positionMs);
    audio.playing = (this.timeline?.segments.length ?? 0) > 0;
  });
  constructor(readonly options: { onClockChange: () => void }) {
    audio.instances.push(this);
  }
  currentTimeMs = () => this.wrap(audio.positionMs);
  dispose() {}
  isClockRunning = () => audio.clockRunning;
  isPlaying = () => audio.playing;
  pause = vi.fn(() => {
    audio.playing = false;
  });
  seekMs = vi.fn((ms: number) => {
    audio.positionMs = this.wrap(ms);
  });
  setRange = vi.fn((range: FakeRange | null) => {
    this.range = range;
  });
  setRate: ReturnType<typeof vi.fn> = vi.fn();
  setVolume: ReturnType<typeof vi.fn> = vi.fn();
  loadSource = vi.fn(async (_id: string, _input: unknown) => {});
  removeSource: ReturnType<typeof vi.fn> = vi.fn();
  wrap(ms: number) {
    const durationMs = this.timeline
      ? (this.timeline.durationFrames * 1_000) / this.timeline.fps
      : 0;
    const range =
      this.range ?? (durationMs > 0 ? { endMs: durationMs, loop: true, startMs: 0 } : null);
    if (!range) {
      return ms;
    }
    const length = range.endMs - range.startMs;
    return range.loop
      ? range.startMs + ((((ms - range.startMs) % length) + length) % length)
      : Math.min(Math.max(ms, range.startMs), range.endMs);
  }
}
