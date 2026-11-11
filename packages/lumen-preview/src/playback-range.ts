export const MIN_PLAYBACK_RATE = 0.25;
export const MAX_PLAYBACK_RATE = 4;

/** A section of the timeline, in frames. `out` is exclusive: the range plays frames `in` to `out - 1`. */
export interface LumenPreviewRange {
  in: number;
  out: number;
}

/** What the playback options ask for, with defaults filled in. */
export interface PlaybackSettings {
  loop: boolean;
  muted: boolean;
  range: LumenPreviewRange | null;
  rate: number;
  volume: number;
}

/** The frames `[inFrame, outFrame)` a playback covers on a loaded timeline. */
export interface PlaybackRange {
  inFrame: number;
  loop: boolean;
  outFrame: number;
}

export const DEFAULT_PLAYBACK: PlaybackSettings = {
  loop: false,
  muted: false,
  range: null,
  rate: 1,
  volume: 1,
};

/** Throws a `RangeError` naming the first option with a value that can't be used. */
export function readPlaybackSettings(inputs: {
  [Key in keyof PlaybackSettings]?: PlaybackSettings[Key] | undefined;
}): PlaybackSettings {
  const {
    loop = DEFAULT_PLAYBACK.loop,
    muted = DEFAULT_PLAYBACK.muted,
    range = DEFAULT_PLAYBACK.range,
    rate = DEFAULT_PLAYBACK.rate,
    volume = DEFAULT_PLAYBACK.volume,
  } = inputs;
  if (!Number.isFinite(rate) || rate < MIN_PLAYBACK_RATE || rate > MAX_PLAYBACK_RATE) {
    throw new RangeError(
      `rate must be a number from ${MIN_PLAYBACK_RATE} to ${MAX_PLAYBACK_RATE}, got ${String(rate)}`,
    );
  }
  if (!Number.isFinite(volume) || volume < 0 || volume > 1) {
    throw new RangeError(`volume must be a number from 0 to 1, got ${String(volume)}`);
  }
  if (range !== null) {
    const { in: inFrame, out: outFrame } = range;
    if (!Number.isInteger(inFrame) || !Number.isInteger(outFrame) || inFrame < 0) {
      throw new RangeError(
        `range.in and range.out must be whole frame numbers, with in at least 0, got ${String(inFrame)} and ${String(outFrame)}`,
      );
    }
    if (inFrame >= outFrame) {
      throw new RangeError(`range.in must be less than range.out, got ${inFrame} and ${outFrame}`);
    }
  }
  return { loop, muted, range: range && { in: range.in, out: range.out }, rate, volume };
}

/**
 * The range a timeline of `totalFrames` plays: `out` past the end is cut to it, and a range that
 * starts past the end is ignored, which leaves the whole timeline.
 */
export function resolvePlaybackRange(
  range: LumenPreviewRange | null,
  loop: boolean,
  totalFrames: number,
): PlaybackRange {
  if (!range || range.in >= totalFrames) {
    return { inFrame: 0, loop, outFrame: totalFrames };
  }
  return { inFrame: range.in, loop, outFrame: Math.min(range.out, totalFrames) };
}

/** Why a requested range doesn't fit a timeline, or null when it does. */
export function describeRangeMisfit(
  range: LumenPreviewRange | null,
  totalFrames: number,
): string | null {
  if (!range) {
    return null;
  }
  if (range.in >= totalFrames) {
    return `range.in is ${range.in}, but the composition has ${totalFrames} frames; playing all of them`;
  }
  return range.out > totalFrames
    ? `range.out is ${range.out}, but the composition has ${totalFrames} frames; playing to the end`
    : null;
}

/** The frame a playback at absolute frame position `framePosition` shows. */
export function rangeFrame(framePosition: number, range: PlaybackRange): number {
  return range.loop
    ? range.inFrame + mod(framePosition - range.inFrame, range.outFrame - range.inFrame)
    : Math.min(Math.max(framePosition, range.inFrame), range.outFrame - 1);
}

/** A time in the timeline, wrapped round a looping range or held inside one that isn't. */
export function rangeMs(positionMs: number, range: PlaybackRange, fps: number): number {
  const startMs = (range.inFrame * 1_000) / fps;
  const endMs = (range.outFrame * 1_000) / fps;
  return range.loop
    ? startMs + mod(positionMs - startMs, endMs - startMs)
    : Math.min(Math.max(positionMs, startMs), endMs);
}

export function rangeLengthMs(range: PlaybackRange, fps: number): number {
  return ((range.outFrame - range.inFrame) * 1_000) / fps;
}

export function mod(value: number, length: number): number {
  return ((value % length) + length) % length;
}
