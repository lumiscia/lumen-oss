import type {
  Color,
  CompositionNode,
  Connection,
  LumenComposition,
  NodeKind,
} from "@lumiscia/lumen-types";

import type {
  AudioClipOptions,
  AudioClipInput,
  AudioTrackOptions,
  AudioTimelineInput,
  AudioTrackInput,
  AudioTrackReference,
  CompositionOptions,
  ConnectOptions,
  NodeInput,
  NodeReference,
  RenderSettingsInput,
  Size,
  TimelineInput,
} from "./types.js";

const defaultRenderSettings = {
  background_color: [0, 0, 0, 0] as const,
  height: 1080,
  width: 1920,
} satisfies LumenComposition["render_settings"];

const defaultTimeline = {
  duration_frames: 1,
  fps: 24,
} satisfies LumenComposition["timeline"];

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
  track?: AudioTrackReference;
};

export class Composition {
  readonly #connections: Connection[] = [];
  readonly #nodes: CompositionNode[] = [];
  #audio: AudioTimelineInput | undefined;
  #lumenSchemaVersion: string | undefined;
  #metadata: LumenComposition["metadata"] | undefined;
  #renderSettings: LumenComposition["render_settings"];
  #schemaVersion: string | undefined;
  #timeline: LumenComposition["timeline"];

  constructor(options: CompositionOptions = {}) {
    this.#audio = audioTimeline(options.audio);
    this.#metadata = options.metadata;
    this.#lumenSchemaVersion = options.lumenSchemaVersion;
    this.#schemaVersion = options.schemaVersion;
    this.#renderSettings = renderSettings(options.renderSettings);
    this.#timeline = timeline(options.timeline);
  }

  addNode<TKind extends NodeKind>(node: NodeInput<TKind>): CompositionNode<TKind> {
    const id = node.id ?? this.#nodes.length;
    const nextNode = { ...node, id } as CompositionNode<TKind>;

    this.#nodes.push(nextNode);
    return nextNode;
  }

  connect(from: NodeReference, to: NodeReference, options: ConnectOptions = {}): this {
    const connection: Connection = {
      from_node: nodeId(from),
      to_node: nodeId(to),
      to_port: options.toPort ?? "input",
    };

    this.#connections.push(
      options.fromPort === undefined
        ? connection
        : {
            ...connection,
            from_port: options.fromPort,
          },
    );

    return this;
  }

  setSize(size: Size): this {
    this.#renderSettings = {
      ...this.#renderSettings,
      height: size.height,
      width: size.width,
    };
    return this;
  }

  setBackgroundColor(color: Color): this {
    this.#renderSettings = {
      ...this.#renderSettings,
      background_color: color,
    };
    return this;
  }

  setFps(fps: number): this {
    this.#timeline = {
      ...this.#timeline,
      fps,
    };
    return this;
  }

  setDurationFrames(durationFrames: number): this {
    this.#timeline = {
      ...this.#timeline,
      duration_frames: durationFrames,
    };
    return this;
  }

  setDurationSeconds(durationSeconds: number): this {
    return this.setDurationFrames(Math.ceil(durationSeconds * this.#timeline.fps));
  }

  setMetadata(metadata: LumenComposition["metadata"]): this {
    this.#metadata = metadata;
    return this;
  }

  setAudio(audio: AudioTimelineInput | undefined): this {
    this.#audio = audioTimeline(audio);
    return this;
  }

  addAudioTrack(id: string, options?: Omit<AudioTrackOptions, "id">): AudioTrackInput;
  addAudioTrack(track?: AudioTrackOptions): AudioTrackInput;
  addAudioTrack(
    input: AudioTrackOptions | string = {},
    options: Omit<AudioTrackOptions, "id"> = {},
  ): AudioTrackInput {
    const audio = this.#audio ?? emptyAudioTimeline();
    const track = audioTrack(input, options, audio.tracks.length);
    this.#audio = {
      ...audio,
      tracks: [...audio.tracks, track],
    };
    return track;
  }

  addAudioClip(track: AudioTrackReference, clip: Omit<AudioClipOptions, "track">): AudioClipInput;
  addAudioClip(clip: AudioClipInput | AudioClipOptions): AudioClipInput;
  addAudioClip(
    input: AudioClipInput | AudioClipOptions | AudioTrackReference,
    options?: Omit<AudioClipOptions, "track">,
  ): AudioClipInput {
    const audio = this.#audio ?? emptyAudioTimeline();
    const clip = audioClip(input, options, audio.clips.length, this.#timeline.fps);
    this.#audio = {
      ...audio,
      clips: [...audio.clips, clip],
    };
    return clip;
  }

  toJSON(): LumenComposition {
    return {
      connections: [...this.#connections],
      nodes: [...this.#nodes],
      render_settings: this.#renderSettings,
      timeline: this.#timeline,
      ...(this.#audio !== undefined ? { audio: this.#audio } : {}),
      ...(this.#lumenSchemaVersion !== undefined
        ? { lumenSchemaVersion: this.#lumenSchemaVersion }
        : {}),
      ...(this.#metadata !== undefined ? { metadata: this.#metadata } : {}),
      ...(this.#schemaVersion !== undefined ? { schemaVersion: this.#schemaVersion } : {}),
    };
  }
}

function nodeId(node: NodeReference): number {
  return typeof node === "number" ? node : node.id;
}

function renderSettings(
  input: RenderSettingsInput | undefined,
): LumenComposition["render_settings"] {
  return {
    ...defaultRenderSettings,
    ...input,
  };
}

function timeline(input: TimelineInput | undefined): LumenComposition["timeline"] {
  const fps = input?.fps ?? defaultTimeline.fps;
  return {
    ...defaultTimeline,
    ...input,
    duration_frames:
      input?.durationSeconds === undefined
        ? (input?.duration_frames ?? defaultTimeline.duration_frames)
        : Math.ceil(input.durationSeconds * fps),
    fps,
  };
}

function audioTimeline(input: AudioTimelineInput | undefined): AudioTimelineInput | undefined {
  if (input === undefined) {
    return undefined;
  }

  return {
    ...input,
    clips: [...input.clips],
    tracks: [...input.tracks],
  };
}

function audioTrack(
  input: AudioTrackOptions | string,
  options: Omit<AudioTrackOptions, "id">,
  index: number,
): AudioTrackInput {
  if (typeof input === "string") {
    return {
      ...options,
      id: input,
    };
  }

  return {
    ...input,
    id: input.id ?? `audio-track-${index + 1}`,
  };
}

function audioClip(
  input: AudioClipInput | AudioClipOptions | AudioTrackReference,
  options: Omit<AudioClipOptions, "track"> | undefined,
  index: number,
  fps: number,
): AudioClipInput {
  if (options !== undefined) {
    return audioClipFromOptions(
      { ...options, track: input as AudioTrackReference } as AudioClipOptions,
      index,
      fps,
    );
  }

  if (isAudioClipOptions(input)) {
    return audioClipFromOptions(input, index, fps);
  }

  if (isAudioClipInput(input)) {
    return input;
  }

  throw new Error("addAudioClip requires a clip input or a track reference with clip options");
}

function audioClipFromOptions(input: AudioClipOptions, index: number, fps: number): AudioClipInput {
  const clip: MutableAudioClipInput = {
    ...input,
    id: input.id ?? `audio-clip-${index + 1}`,
    source_id: input.sourceId,
    track_id: audioTrackId(input.track),
  };

  setAudioTime(clip, "start_ms", input.startMs, input.startSeconds);
  setAudioTime(clip, "duration_ms", input.durationMs, input.durationSeconds);
  setAudioTime(clip, "source_start_ms", input.sourceStartMs, input.sourceStartSeconds);

  if (clip.start_ms === undefined && input.startFrame !== undefined) {
    clip.start_ms = framesToMs(input.startFrame, fps);
  }
  if (clip.duration_ms === undefined && input.durationFrames !== undefined) {
    clip.duration_ms = framesToMs(input.durationFrames, fps);
  }

  deleteExtraAudioClipInputFields(clip);
  return clip;
}

function setAudioTime(
  clip: MutableAudioClipInput,
  key: "duration_ms" | "source_start_ms" | "start_ms",
  milliseconds: number | undefined,
  seconds: number | undefined,
): void {
  if (milliseconds !== undefined) {
    clip[key] = milliseconds;
    return;
  }

  if (seconds !== undefined) {
    clip[key] = Math.round(seconds * 1_000);
  }
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
  delete clip.track;
}

function isAudioClipOptions(
  input: AudioClipInput | AudioClipOptions | AudioTrackReference,
): input is AudioClipOptions {
  return typeof input !== "string" && "sourceId" in input && "track" in input;
}

function isAudioClipInput(
  input: AudioClipInput | AudioClipOptions | AudioTrackReference,
): input is AudioClipInput {
  return typeof input !== "string" && "source_id" in input && "track_id" in input;
}

function audioTrackId(track: AudioTrackReference): string {
  return typeof track === "string" ? track : track.id;
}

function framesToMs(frames: number, fps: number): number {
  return Math.round((frames / fps) * 1_000);
}

function emptyAudioTimeline(): AudioTimelineInput {
  return {
    clips: [],
    tracks: [],
  };
}
