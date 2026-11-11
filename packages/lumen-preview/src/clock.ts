import type { PlaybackRange } from "./playback-range.js";

/** A playback position that moves by itself: `originMs` at `atEpochMs`, advancing at `rate`. */
export interface PreviewClock {
  atEpochMs: number;
  originMs: number;
  rate: number;
}

/** Milliseconds that mean the same on the page and in its workers. */
export function epochNow(): number {
  return performance.timeOrigin + performance.now();
}

export function clockPositionMs(clock: PreviewClock, nowEpochMs: number): number {
  return clock.originMs + (nowEpochMs - clock.atEpochMs) * clock.rate;
}

/** A parked clock outside the range waits on its nearest frame. A running one is left to the range. */
export function fitClock(clock: PreviewClock, range: PlaybackRange, fps: number): PreviewClock {
  if (clock.rate !== 0) {
    return clock;
  }
  const firstFrameMs = (range.inFrame * 1_000) / fps;
  const lastFrameMs = (Math.max(range.outFrame - 1, 0) * 1_000) / fps;
  const originMs = Math.min(Math.max(clock.originMs, firstFrameMs), lastFrameMs);
  return originMs === clock.originMs ? clock : { ...clock, originMs };
}
