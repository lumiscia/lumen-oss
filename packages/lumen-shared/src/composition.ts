import type {
  Color,
  CompositionNode,
  CompositionTemplate,
  Connection,
  LumenComposition,
  NodeKind,
} from "@lumiscia/lumen-types";

import { AudioTrack, audioTrackTimeline } from "./audio.js";
import { Expr, type FunctionDefinition } from "./expression.js";
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
  fps: 30,
} satisfies LumenComposition["timeline"];

interface ClipTemplate {
  readonly name: string;
  readonly template: CompositionTemplate;
  readonly composition: Composition;
}

/** The template a clip node input plays, which `addNode` registers. Spreading the input keeps it. */
export const clipTemplate = Symbol("clipTemplate");

export class Composition {
  readonly #compositions = new Map<string, ClipTemplate>();
  readonly #connections: Connection[] = [];
  readonly #functions = new Map<string, FunctionDefinition>();
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

  addNode<TKind extends NodeKind>(
    node: NodeInput<TKind> & { readonly type: TKind },
  ): CompositionNode<TKind> {
    const { [clipTemplate]: template, ...rest } = node as NodeInput<TKind> & {
      readonly [clipTemplate]?: ClipTemplate;
    };
    const input = rest as NodeInput<TKind>;
    const id = input.id ?? this.#allocateNodeId();
    if (this.#nodeIds.has(id)) {
      throw new Error(`Node \`${id}\` already exists.`);
    }
    if (template !== undefined) {
      this.#addComposition(template);
    }

    const nextNode = {
      ...input,
      id,
      ...(input.params === undefined ? {} : { params: lowerParams(input.params) }),
    } as CompositionNode<TKind>;

    this.#nodeIds.add(id);
    this.#nodes.push(nextNode);
    return nextNode;
  }

  #addComposition(template: ClipTemplate): void {
    const existing = this.#compositions.get(template.name);
    if (existing === template) {
      return;
    }
    if (existing !== undefined) {
      throw new Error(`Composition \`${template.name}\` already exists.`);
    }

    this.#compositions.set(template.name, template);
    for (const nested of template.composition.#compositions.values()) {
      this.#addComposition(nested);
    }
    for (const definition of template.composition.#functions.values()) {
      this.addFunction(definition);
    }
  }

  #allocateNodeId(): number {
    while (this.#nodeIds.has(this.#nextNodeId)) {
      this.#nextNodeId += 1;
    }

    const id = this.#nextNodeId;
    this.#nextNodeId += 1;
    return id;
  }

  /** Adds a function from `fn` to the composition's `functions`. */
  addFunction(definition: FunctionDefinition): this {
    const name = functionName(definition);
    const existing = this.#functions.get(name);
    if (existing !== undefined && existing !== definition) {
      throw new Error(`Function \`${name}\` already exists.`);
    }

    for (const other of [...this.#functions.values(), definition]) {
      const clash = functionParams(other).find(
        (param) => param === name || this.#functions.has(param),
      );
      if (clash !== undefined) {
        throw new Error(
          `Parameter \`${clash}\` of \`${functionName(other)}\` has the same name as a function.`,
        );
      }
    }

    this.#functions.set(name, definition);
    return this;
  }

  connect<TKind extends NodeKind>(
    from: NodeReference,
    to: number | { readonly id: number; readonly type?: TKind },
    options: ConnectOptions<TKind>,
  ): this {
    const connection: Connection = {
      from_node: nodeId(from),
      to_node: nodeId(to),
      to_port: options.toPort,
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
    return this.setDurationFrames(
      Math.ceil(durationSeconds * (this.#timeline.fps ?? defaultTimeline.fps)),
    );
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
      ...(this.#compositions.size > 0
        ? {
            compositions: Object.fromEntries(
              [...this.#compositions.values()].map(({ name, template }) => [name, template]),
            ),
          }
        : {}),
      connections: [...this.#connections],
      ...(this.#functions.size > 0
        ? {
            functions: Object.fromEntries(
              [...this.#functions.values()].map(({ signature, body }) => [signature, body]),
            ),
          }
        : {}),
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

function lowerParams(params: object): Record<string, unknown> {
  return Object.fromEntries(
    Object.entries(params).map(([name, value]) => [
      name,
      value instanceof Expr ? value.toString() : value,
    ]),
  );
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

function functionName(definition: FunctionDefinition): string {
  return definition.signature.slice(0, definition.signature.indexOf("("));
}

function functionParams(definition: FunctionDefinition): string[] {
  const { signature } = definition;
  return signature
    .slice(signature.indexOf("(") + 1, signature.lastIndexOf(")"))
    .split(",")
    .map((param) => param.trim())
    .filter((param) => param !== "");
}
