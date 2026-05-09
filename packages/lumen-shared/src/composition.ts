import type {
  Color,
  CompositionNode,
  Connection,
  LumenComposition,
  NodeKind,
} from "@lumiscia/lumen-types";

import type {
  CompositionOptions,
  ConnectOptions,
  MediaLoopMode,
  MediaSourceKind,
  MediaSourceOptions,
  NodeInput,
  NodeReference,
  RenderSettingsInput,
  Size,
  SolidColorOptions,
  TextOptions,
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
  readonly #nodes: CompositionNode[] = [];
  #lumenSchemaVersion: string | undefined;
  #metadata: LumenComposition["metadata"] | undefined;
  #renderSettings: LumenComposition["render_settings"];
  #schemaVersion: string | undefined;
  #timeline: LumenComposition["timeline"];

  constructor(options: CompositionOptions = {}) {
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

  addMediaSource(options: MediaSourceOptions): CompositionNode<"media_in"> {
    return this.addNode<"media_in">({
      ...(options.id !== undefined ? { id: options.id } : {}),
      type: "media_in",
      properties: {
        kind: mediaSourceKind(options.kind ?? "image"),
        source: String(options.source),
        ...(options.rangeStart !== undefined ? { range_start: options.rangeStart } : {}),
        ...(options.rangeEnd !== undefined ? { range_end: options.rangeEnd } : {}),
        ...(options.speed !== undefined ? { speed: options.speed } : {}),
        ...(options.loop !== undefined ? { loop_mode: mediaLoopMode(options.loop) } : {}),
      },
    });
  }

  addImage(source: string | URL, options: Omit<MediaSourceOptions, "kind" | "source"> = {}): CompositionNode<"media_in"> {
    return this.addMediaSource({
      ...options,
      kind: "image",
      source,
    });
  }

  addVideo(source: string | URL, options: Omit<MediaSourceOptions, "kind" | "source"> = {}): CompositionNode<"media_in"> {
    return this.addMediaSource({
      ...options,
      kind: "video",
      source,
    });
  }

  addSolidColor(options: SolidColorOptions): CompositionNode<"solid_color"> {
    return this.addNode<"solid_color">({
      ...(options.id !== undefined ? { id: options.id } : {}),
      type: "solid_color",
      properties: {
        color: options.color,
        height: options.height,
        width: options.width,
      },
    });
  }

  addText(options: TextOptions): CompositionNode<"text"> {
    return this.addNode<"text">({
      ...(options.id !== undefined ? { id: options.id } : {}),
      type: "text",
      properties: {
        content: options.content,
        ...(options.fontFamily !== undefined ? { font_family: options.fontFamily } : {}),
        ...(options.fontSize !== undefined ? { font_size: options.fontSize } : {}),
        ...(options.fontWeight !== undefined ? { font_weight: options.fontWeight } : {}),
        ...(options.fontStyle !== undefined ? { font_style: options.fontStyle } : {}),
        ...(options.maxWidth !== undefined ? { max_width: options.maxWidth } : {}),
        ...(options.color !== undefined ? { color: options.color } : {}),
        ...(options.alignHorizontal !== undefined
          ? { alignment_horizontal: options.alignHorizontal }
          : {}),
        ...(options.alignVertical !== undefined ? { alignment_vertical: options.alignVertical } : {}),
      },
    });
  }

  addOutput(source?: NodeReference): CompositionNode<"media_output"> {
    const output = this.addNode<"media_output">({
      type: "media_output",
    });

    if (source !== undefined) {
      this.connect(source, output, { toPort: "source" });
    }

    return output;
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

  toJSON(): LumenComposition {
    return {
      connections: [...this.#connections],
      nodes: [...this.#nodes],
      render_settings: this.#renderSettings,
      timeline: this.#timeline,
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

function renderSettings(input: RenderSettingsInput | undefined): LumenComposition["render_settings"] {
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

function mediaSourceKind(kind: MediaSourceKind): number {
  switch (kind) {
    case "image":
      return 0;
    case "video":
      return 1;
  }
}

function mediaLoopMode(loop: MediaLoopMode | boolean): number {
  if (loop === true || loop === "loop") {
    return 1;
  }

  return 0;
}
