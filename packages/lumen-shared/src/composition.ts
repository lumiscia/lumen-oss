import type {
  Color,
  CompositionNode,
  Connection,
  LumenComposition,
  NodeKind,
} from "@lumiscia/lumen-types";

import { AudioTrack, audioTrackTimeline } from "./audio.js";
import type {
  AudioTimelineInput,
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

export class Composition {
  readonly #connections: Connection[] = [];
  readonly #nodeIds = new Set<number>();
  readonly #nodes: CompositionNode[] = [];
  #nextNodeId = 0;
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
    const id = node.id ?? this.#allocateNodeId();
    if (this.#nodeIds.has(id)) {
      throw new Error(`Node \`${id}\` already exists.`);
    }

    const nextNode = { ...node, id } as CompositionNode<TKind>;

    this.#nodeIds.add(id);
    this.#nodes.push(nextNode);
    return nextNode;
  }

  #allocateNodeId(): number {
    while (this.#nodeIds.has(this.#nextNodeId)) {
      this.#nextNodeId += 1;
    }

    const id = this.#nextNodeId;
    this.#nextNodeId += 1;
    return id;
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

  addAudioTrack(track: AudioTrack): this {
    const audio = this.#audio ?? emptyAudioTimeline();
    const timeline = audioTrackTimeline(track);
    if (audio.tracks.some((existingTrack) => existingTrack.id === timeline.track.id)) {
      throw new Error(`Audio track \`${timeline.track.id}\` already exists.`);
    }

    this.#audio = {
      ...audio,
      clips: [...audio.clips, ...timeline.clips],
      tracks: [...audio.tracks, timeline.track],
    };
    return this;
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

function emptyAudioTimeline(): AudioTimelineInput {
  return {
    clips: [],
    tracks: [],
  };
}
