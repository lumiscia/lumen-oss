import type {
  CompositionNode,
  CompositionTemplate,
  Connection,
  LumenComposition,
  NodeInputPortKind,
  NodeInputPortName,
  NodeKind,
} from "@lumiscia/lumen-types";

import { Expr, type FunctionDefinition } from "./expression.js";
import type {
  AudioClipOptions,
  AudioTrackOptions,
  CompositionOptions,
  ConnectOptions,
  NodeInput,
  NodeReference,
  RenderSettingsInput,
  TimelineInput,
} from "./types.js";

const defaultRenderSettings = {
  height: 1080,
  width: 1920,
} satisfies LumenComposition["render_settings"];

const defaultAudioTrackId = "main";

const defaultTimeline = {
  duration_frames: 1,
  fps: 30,
} satisfies LumenComposition["timeline"];

/** A template with the nested templates and functions its `build` needed, as of its definition. */
export interface ClipTemplate {
  readonly name: string;
  readonly template: CompositionTemplate;
  readonly compositions: readonly ClipTemplate[];
  readonly functions: readonly FunctionDefinition[];
}

/** The template a clip node input plays, which `addNode` registers. Spreading the input keeps it. */
export const clipTemplate = Symbol("clipTemplate");

let dependenciesOf: (composition: Composition) => Pick<ClipTemplate, "compositions" | "functions">;

/** Captures what `composition`, a template's built nodes, has registered so far. */
export function captureClipTemplate(
  name: string,
  template: CompositionTemplate,
  composition: Composition,
): ClipTemplate {
  return { name, template, ...dependenciesOf(composition) };
}

export class Composition {
  readonly #compositions = new Map<string, ClipTemplate>();
  readonly #connections: Connection[] = [];
  readonly #functions = new Map<string, FunctionDefinition>();
  readonly #handles = new WeakSet<object>();
  readonly #nodeIds = new Set<number>();
  readonly #nodes: CompositionNode[] = [];
  #nextNodeId = 0;
  #audio: LumenComposition["audio"];
  #lumenSchemaVersion: LumenComposition["lumenSchemaVersion"] | undefined;
  #metadata: LumenComposition["metadata"] | undefined;
  #renderSettings: LumenComposition["render_settings"];
  #timeline: LumenComposition["timeline"];

  static {
    dependenciesOf = (composition) => ({
      compositions: [...composition.#compositions.values()],
      functions: [...composition.#functions.values()],
    });
  }

  constructor(options: CompositionOptions = {}) {
    this.#audio = options.audio && {
      ...options.audio,
      clips: options.audio.clips.map((clip) => ({ ...clip })),
      tracks: options.audio.tracks.map((track) => ({ ...track })),
    };
    this.#metadata = options.metadata;
    this.#lumenSchemaVersion = options.lumenSchemaVersion;
    this.#renderSettings = renderSettings(options.renderSettings);
    this.#timeline = timeline(options.timeline);
  }

  /**
   * Adds a node. `inputs` connects other nodes of this composition to its ports in order, as
   * `connect` would after it; a variadic port such as `layers` takes an array.
   */
  addNode<TKind extends NodeKind>(
    node: NodeInput<TKind> & { readonly type: TKind },
  ): CompositionNode<TKind> {
    const {
      [clipTemplate]: template,
      inputs,
      ...rest
    } = node as NodeInput<TKind> & {
      readonly [clipTemplate]?: ClipTemplate;
    };
    const input = rest as unknown as NodeInput<TKind>;
    const id = input.id ?? this.#nextFreeNodeId();
    if (this.#nodeIds.has(id)) {
      throw new Error(`Node \`${id}\` already exists.`);
    }
    const connections = Object.entries(
      (inputs ?? {}) as Readonly<Record<string, NodeReference | readonly NodeReference[]>>,
    ).flatMap(([toPort, from]) =>
      [from].flat().map((reference): Connection => ({
        from_node: this.#ownedNodeId(reference),
        to_node: id,
        to_port: toPort,
      })),
    );
    const addTemplate = template === undefined ? undefined : this.#planTemplate(template);
    const nextNode = {
      ...input,
      id,
      ...(input.params === undefined ? {} : { params: lower(input.params) }),
    } as CompositionNode<TKind>;

    // Everything that can throw is above, so a failed addition leaves the composition as it was.
    addTemplate?.();
    this.#nodeIds.add(id);
    if (input.id === undefined) {
      this.#nextNodeId = id + 1;
    }
    this.#nodes.push(nextNode);
    this.#handles.add(nextNode);
    for (const connection of connections) {
      this.#connections.push(connection);
    }
    return nextNode;
  }

  /** Checks that `template` fits and returns the function that registers it. */
  #planTemplate(template: ClipTemplate): () => void {
    const added = new Map<string, ClipTemplate>();
    for (const next of [template, ...template.compositions]) {
      const existing = this.#compositions.get(next.name) ?? added.get(next.name);
      if (existing === next) {
        continue;
      }
      if (existing !== undefined) {
        throw new Error(`Composition \`${next.name}\` already exists.`);
      }
      added.set(next.name, next);
    }
    const functions = template.functions.filter((definition) => this.#isNewFunction(definition));

    return () => {
      for (const [name, next] of added) {
        this.#compositions.set(name, next);
      }
      for (const definition of functions) {
        this.#functions.set(functionName(definition), definition);
      }
    };
  }

  #nextFreeNodeId(): number {
    let id = this.#nextNodeId;
    while (this.#nodeIds.has(id)) {
      id += 1;
    }
    return id;
  }

  /** A node handle must come from this composition. Ids are the engine's to check. */
  #ownedNodeId(node: NodeReference): number {
    if (typeof node !== "number" && !this.#handles.has(node)) {
      throw new Error(`Node \`${node.id}\` is not a node of this composition.`);
    }
    return nodeId(node);
  }

  /** Adds a function from `fn` to the composition's `functions`. */
  addFunction(definition: FunctionDefinition): this {
    if (this.#isNewFunction(definition)) {
      this.#functions.set(functionName(definition), definition);
    }
    return this;
  }

  /** Whether `definition` is yet to be added. A different function by the same name throws. */
  #isNewFunction(definition: FunctionDefinition): boolean {
    const name = functionName(definition);
    const existing = this.#functions.get(name);
    if (existing !== undefined && existing !== definition) {
      throw new Error(`Function \`${name}\` already exists.`);
    }
    return existing === undefined;
  }

  connect<
    TKind extends NodeKind,
    TPort extends NodeInputPortName<TKind> = NodeInputPortName<TKind>,
  >(
    from: NodeReference<NodeInputPortKind<NoInfer<TKind>, NoInfer<TPort>>>,
    to: number | { readonly id: number; readonly type?: TKind },
    options: ConnectOptions<NoInfer<TKind>, TPort>,
  ): this {
    this.#connections.push({
      from_node: this.#ownedNodeId(from),
      to_node: this.#ownedNodeId(to),
      to_port: options.toPort,
    });
    return this;
  }

  /**
   * Adds an audio clip, with times in seconds. A `trackId` that isn't declared becomes a plain
   * track, so declare a track with `addAudioTrack` first to mute it or set its gain.
   */
  addAudio(options: AudioClipOptions): this {
    const { source, start, duration, offset, gain, id, name } = options;
    const trackId = options.trackId ?? defaultAudioTrackId;
    const audio = this.#audio ?? { clips: [], tracks: [] };
    const taken = new Set(audio.clips.map((clip) => clip.id));
    let number = audio.clips.filter((clip) => clip.track_id === trackId).length + 1;
    while (taken.has(`${trackId}-clip-${number}`)) {
      number += 1;
    }
    this.#audio = {
      ...audio,
      clips: [
        ...audio.clips,
        {
          duration_ms: milliseconds(duration),
          id: id ?? `${trackId}-clip-${number}`,
          ...(name === undefined ? {} : { name }),
          source_id: source,
          ...(offset === undefined ? {} : { source_start_ms: milliseconds(offset) }),
          start_ms: milliseconds(start),
          track_id: trackId,
          ...(gain === undefined ? {} : { volume: gain }),
        },
      ],
      tracks: audio.tracks.some((track) => track.id === trackId)
        ? audio.tracks
        : [...audio.tracks, { id: trackId }],
    };
    return this;
  }

  /** Declares an audio track with its name, mute, solo and gain. */
  addAudioTrack(options: AudioTrackOptions): this {
    const { id, name, muted, solo, gain } = options;
    const audio = this.#audio ?? { clips: [], tracks: [] };
    if (audio.tracks.some((track) => track.id === id)) {
      throw new Error(`Audio track \`${id}\` already exists.`);
    }

    this.#audio = {
      ...audio,
      tracks: [
        ...audio.tracks,
        {
          id,
          ...(muted === undefined ? {} : { muted }),
          ...(name === undefined ? {} : { name }),
          ...(solo === undefined ? {} : { solo }),
          ...(gain === undefined ? {} : { volume: gain }),
        },
      ],
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
    };
  }
}

/** Turns every `Expr` in plain objects and arrays into its `=...` string. */
function lower(value: unknown): unknown {
  if (value instanceof Expr) {
    return value.toString();
  }
  if (Array.isArray(value)) {
    return value.map(lower);
  }
  if (isPlainObject(value)) {
    return Object.fromEntries(Object.entries(value).map(([key, entry]) => [key, lower(entry)]));
  }
  return value;
}

function isPlainObject(value: unknown): value is object {
  if (typeof value !== "object" || value === null) {
    return false;
  }
  const prototype = Object.getPrototypeOf(value);
  return prototype === Object.prototype || prototype === null;
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

function timeline(input: TimelineInput = {}): LumenComposition["timeline"] {
  const {
    duration_frames: durationFrames = defaultTimeline.duration_frames,
    durationSeconds,
    fps = defaultTimeline.fps,
    ...rest
  } = input;
  return {
    ...rest,
    duration_frames:
      durationSeconds === undefined ? durationFrames : Math.ceil(durationSeconds * fps),
    fps,
  };
}

/** Audio times are whole milliseconds on the wire, whatever the composition's fps. */
function milliseconds(seconds: number): number {
  return Math.round(seconds * 1_000);
}

function functionName(definition: FunctionDefinition): string {
  return definition.signature.slice(0, definition.signature.indexOf("("));
}
