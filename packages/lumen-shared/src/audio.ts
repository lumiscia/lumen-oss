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
  const sourceStartMs = audioTimeMs(input.sourceStartMs, input.sourceStartSeconds);
  return {
    ...rest,
    duration_ms: audioTimeMs(input.durationMs, input.durationSeconds) ?? 0,
    id: input.id ?? `${trackId}-clip-${index + 1}`,
    source_id: input.sourceId,
    ...(sourceStartMs === undefined ? {} : { source_start_ms: sourceStartMs }),
    start_ms: audioTimeMs(input.startMs, input.startSeconds) ?? 0,
    track_id: trackId,
  };
}

function audioClipFromInput(input: AudioClipInput, trackId: string): AudioClipInput {
  if (input.track_id !== trackId) {
    throw new Error(
      `Audio clip \`${input.id}\` belongs to track \`${input.track_id}\`, not \`${trackId}\`.`,
    );
  }

  return { ...input };
}

function assertAudioClipTiming(clip: AudioClipInput): void {
  if (clip.duration_ms <= 0) {
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
  return {
    start: clip.start_ms,
    end: clip.start_ms + clip.duration_ms,
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
