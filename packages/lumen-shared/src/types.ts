import type {
  Color,
  CompositionNode,
  CompositionNodeInput,
  LumenComposition,
  NodeKind,
} from "@lumiscia/lumen-types";

export interface LumenOptions {
  readonly apiKey: string;
  readonly baseUrl?: string;
  readonly fetch?: typeof fetch;
  readonly websocket?: typeof WebSocket;
}

export interface CompositionOptions {
  readonly metadata?: LumenComposition["metadata"];
  readonly renderSettings?: RenderSettingsInput;
  readonly timeline?: TimelineInput;
  readonly lumenSchemaVersion?: string;
  readonly schemaVersion?: string;
}

export type NodeInput<TKind extends NodeKind = NodeKind> = CompositionNodeInput<TKind>;

export type NodeReference = number | Pick<CompositionNode, "id">;

export interface ConnectOptions {
  readonly fromPort?: string;
  readonly toPort?: string;
}

export interface Size {
  readonly width: number;
  readonly height: number;
}

export type RenderSettingsInput = Partial<LumenComposition["render_settings"]>;

export type TimelineInput = Partial<LumenComposition["timeline"]> & {
  readonly durationSeconds?: number;
};

export type MediaSourceKind = "image" | "video";

export type MediaLoopMode = "none" | "loop";

export interface MediaSourceOptions {
  readonly id?: number;
  readonly kind?: MediaSourceKind;
  readonly source: string | URL;
  readonly rangeStart?: number;
  readonly rangeEnd?: number;
  readonly speed?: number;
  readonly loop?: MediaLoopMode | boolean;
}

export interface SolidColorOptions extends Size {
  readonly id?: number;
  readonly color: Color;
}

export interface TextOptions {
  readonly id?: number;
  readonly content: string;
  readonly fontFamily?: string;
  readonly fontSize?: number;
  readonly fontWeight?: number;
  readonly fontStyle?: number;
  readonly maxWidth?: number;
  readonly color?: Color;
  readonly alignHorizontal?: number;
  readonly alignVertical?: number;
}

export interface RenderOptions {
  readonly signal?: AbortSignal;
  readonly idempotencyKey?: string;
}

export interface RenderResult {
  readonly id?: string;
  readonly error?: LumenApiError;
}

export interface LumenApiError {
  readonly code: string;
  readonly message: string;
  readonly details?: unknown;
}

export type RenderEvent =
  | {
      readonly type: "render.queued";
      readonly renderId: string;
      readonly position?: number;
    }
  | {
      readonly type: "render.started";
      readonly renderId: string;
    }
  | {
      readonly type: "render.progress";
      readonly renderId: string;
      readonly progress: number;
      readonly frame?: number;
      readonly totalFrames?: number;
    }
  | {
      readonly type: "render.completed";
      readonly renderId: string;
      readonly url?: string;
      readonly artifactId?: string;
    }
  | {
      readonly type: "render.failed";
      readonly renderId: string;
      readonly error: LumenApiError;
    };

export interface RenderEventHandlers {
  readonly onEvent?: (event: RenderEvent) => void;
  readonly onError?: (error: unknown) => void;
  readonly onClose?: (event: CloseEvent) => void;
}

export interface RenderEventSubscription {
  readonly close: () => void;
}
