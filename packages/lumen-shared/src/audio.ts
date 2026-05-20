import type { AudioClipInput, AudioClipOptions, AudioTrackInput } from "./types.js";

type MutableAudioClipInput = {
  -readonly [K in keyof AudioClipInput]: AudioClipInput[K];
} & {
  durationFrames?: number;
  durationMs?: number;
  durationSeconds?: number;
  sourceId?: string;
  sourceStartMs?: number;
  sourceStartSeconds?: number;
  startFrame?: number;
  startMs?: number;
  startSeconds?: number;
};

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
  const clip: MutableAudioClipInput = {
    ...input,
    id: input.id ?? `audio-clip-${index + 1}`,
    source_id: input.sourceId,
    start_ms: audioTimeMs(input.startMs, input.startSeconds) ?? 0,
    track_id: trackId,
  };

  const durationMs = audioTimeMs(input.durationMs, input.durationSeconds);
  if (durationMs !== undefined) {
    clip.duration_ms = durationMs;
  }

  const sourceStartMs = audioTimeMs(input.sourceStartMs, input.sourceStartSeconds);
  if (sourceStartMs !== undefined) {
    clip.source_start_ms = sourceStartMs;
  }

  deleteExtraAudioClipInputFields(clip);
  return clip;
}

function audioClipFromInput(input: AudioClipInput, trackId: string): AudioClipInput {
  const clip: MutableAudioClipInput = {
    ...input,
    start_ms: input.start_ms ?? 0,
    track_id: input.track_id,
  };

  if (clip.track_id !== trackId) {
    throw new Error(
      `Audio clip \`${clip.id}\` belongs to track \`${clip.track_id}\`, not \`${trackId}\`.`,
    );
  }

  if (clip.source_start_ms === undefined && typeof clip.source_start_seconds === "number") {
    clip.source_start_ms = Math.round(clip.source_start_seconds * 1_000);
  }
  delete clip.source_start_seconds;

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

function deleteExtraAudioClipInputFields(clip: MutableAudioClipInput): void {
  delete clip.durationFrames;
  delete clip.durationMs;
  delete clip.durationSeconds;
  delete clip.sourceId;
  delete clip.sourceStartMs;
  delete clip.sourceStartSeconds;
  delete clip.startFrame;
  delete clip.startMs;
  delete clip.startSeconds;
}

function isAudioClipOptions(input: AudioClipInput | AudioClipOptions): input is AudioClipOptions {
  return "sourceId" in input;
}
