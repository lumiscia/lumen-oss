import type {
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
  TimelineInput,
} from "./types.js";

const defaultRenderSettings = {
  height: 1080,
  width: 1920,
} satisfies LumenComposition["render_settings"];

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
  #audio: AudioTimelineInput | undefined;
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
    this.#audio = audioTimeline(options.audio);
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
    const id = input.id ?? this.#allocateNodeId();
    if (this.#nodeIds.has(id)) {
      throw new Error(`Node \`${id}\` already exists.`);
    }
    const connections = Object.entries(
      (inputs ?? {}) as Readonly<Record<string, NodeReference | readonly NodeReference[]>>,
    ).flatMap(([toPort, from]) =>
      [from].flat().map((reference): Connection => ({
        from_node: this.#knownNodeId(reference),
        to_node: id,
        to_port: toPort,
      })),
    );
    if (template !== undefined) {
      this.#addTemplate(template);
    }

    const nextNode = {
      ...input,
      id,
      ...(input.params === undefined ? {} : { params: lower(input.params) }),
    } as CompositionNode<TKind>;

    this.#nodeIds.add(id);
    this.#nodes.push(nextNode);
    this.#handles.add(nextNode);
    this.#connections.push(...connections);
    return nextNode;
  }

  #addTemplate(template: ClipTemplate): void {
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

    for (const [name, next] of added) {
      this.#compositions.set(name, next);
    }
    for (const definition of functions) {
      this.#functions.set(functionName(definition), definition);
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

  #knownNodeId(node: NodeReference): number {
    const known = typeof node === "number" ? this.#nodeIds.has(node) : this.#handles.has(node);
    if (!known) {
      throw new Error(`Node \`${nodeId(node)}\` is not a node of this composition.`);
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

  connect<TKind extends NodeKind>(
    from: NodeReference,
    to: number | { readonly id: number; readonly type?: TKind },
    options: ConnectOptions<TKind>,
  ): this {
    this.#connections.push({
      from_node: this.#knownNodeId(from),
      to_node: this.#knownNodeId(to),
      to_port: options.toPort,
    });
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
