import type { AudioClipInput, AudioClipOptions, AudioTrackInput } from "./types.js";

export class AudioTrack {
  readonly #clips: AudioClipInput[] = [];
  readonly #track: AudioTrackInput;

  constructor(track: AudioTrackInput) {
    this.#track = { ...track };
  }

  get id(): string {
    return this.#track.id;
  }

  get clips(): readonly AudioClipInput[] {
    return this.#clips.map((clip) => ({ ...clip }));
  }

  addClip(input: AudioClipInput | AudioClipOptions): AudioClipInput {
    const clip = audioClip(input, this.#track.id, this.#clips.length);
    assertNoAudioClipOverlap(this.#clips, clip);
    this.#clips.push(clip);
    return { ...clip };
  }

  toJSON(): AudioTrackInput {
    return { ...this.#track };
  }
}

export function audioTrackTimeline(track: AudioTrack): {
  clips: readonly AudioClipInput[];
  track: AudioTrackInput;
} {
  return {
    clips: track.clips,
    track: track.toJSON(),
  };
}

function audioClip(
  input: AudioClipInput | AudioClipOptions,
  trackId: string,
  index: number,
): AudioClipInput {
  const clip = isAudioClipOptions(input)
    ? audioClipFromOptions(input, trackId, index)
    : audioClipFromInput(input, trackId);

  assertAudioClipTiming(clip);
  return clip;
}

function audioClipFromOptions(
  input: AudioClipOptions,
  trackId: string,
  index: number,
): AudioClipInput {
  const {
    durationMs: _durationMs,
    durationSeconds: _durationSeconds,
    sourceId: _sourceId,
    sourceStartMs: _sourceStartMs,
    sourceStartSeconds: _sourceStartSeconds,
    startMs: _startMs,
    startSeconds: _startSeconds,
    ...rest
  } = input;
  const durationMs = audioTimeMs(input.durationMs, input.durationSeconds);
  const sourceStartMs = audioTimeMs(input.sourceStartMs, input.sourceStartSeconds);
  return {
    ...rest,
    ...(durationMs === undefined ? {} : { duration_ms: durationMs }),
    id: input.id ?? `audio-clip-${index + 1}`,
    source_id: input.sourceId,
    ...(sourceStartMs === undefined ? {} : { source_start_ms: sourceStartMs }),
    start_ms: audioTimeMs(input.startMs, input.startSeconds) ?? 0,
    track_id: trackId,
  };
}

function audioClipFromInput(input: AudioClipInput, trackId: string): AudioClipInput {
  const sourceStartMs =
    input.source_start_ms ??
    (typeof input.source_start_seconds === "number"
      ? Math.round(input.source_start_seconds * 1_000)
      : undefined);
  const { source_start_seconds: _sourceStartSeconds, ...rest } = input;
  const clip: AudioClipInput = {
    ...rest,
    ...(sourceStartMs === undefined ? {} : { source_start_ms: sourceStartMs }),
    start_ms: input.start_ms ?? 0,
    track_id: input.track_id,
  };

  if (clip.track_id !== trackId) {
    throw new Error(
      `Audio clip \`${clip.id}\` belongs to track \`${clip.track_id}\`, not \`${trackId}\`.`,
    );
  }

  return clip;
}

function assertAudioClipTiming(clip: AudioClipInput): void {
  if (clip.start_frame !== undefined || clip.duration_frames !== undefined) {
    throw new Error(
      `Audio clip \`${clip.id}\` uses frame-based timing. AudioTrack clips must use milliseconds or seconds.`,
    );
  }

  if (clip.duration_ms === undefined || clip.duration_ms <= 0) {
    throw new Error(`Audio clip \`${clip.id}\` must have a duration greater than zero.`);
  }
}

function assertNoAudioClipOverlap(
  existingClips: readonly AudioClipInput[],
  nextClip: AudioClipInput,
): void {
  const nextRange = audioClipRange(nextClip);

  for (const clip of existingClips) {
    const range = audioClipRange(clip);
    if (nextRange.start < range.end && nextRange.end > range.start) {
      throw new Error(
        `Audio clip \`${nextClip.id}\` (${nextRange.start}-${nextRange.end}ms) overlaps ` +
          `\`${clip.id}\` (${range.start}-${range.end}ms).`,
      );
    }
  }
}

function audioClipRange(clip: AudioClipInput): { start: number; end: number } {
  const start = clip.start_ms ?? 0;
  const duration = clip.duration_ms ?? 0;
  return {
    start,
    end: start + duration,
  };
}

function audioTimeMs(
  milliseconds: number | undefined,
  seconds: number | undefined,
): number | undefined {
  if (milliseconds !== undefined) {
    return milliseconds;
  }

  return seconds === undefined ? undefined : Math.round(seconds * 1_000);
}

function isAudioClipOptions(input: AudioClipInput | AudioClipOptions): input is AudioClipOptions {
  return "sourceId" in input;
}
