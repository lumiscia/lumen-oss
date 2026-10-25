import type {
  Color,
  CompositionNode,
  CompositionNodeInput,
  ExpressionString,
  LumenComposition,
  NodeKind,
  Vec2,
} from "@lumiscia/lumen-types";

import type { Expr } from "./expression.js";

export interface LumenOptions {
  readonly apiKey: string;
  readonly baseUrl?: string;
  readonly fetch?: typeof fetch;
  readonly websocket?: typeof WebSocket;
}

export interface CompositionOptions {
  readonly audio?: AudioTimelineInput;
  readonly metadata?: LumenComposition["metadata"];
  readonly renderSettings?: RenderSettingsInput;
  readonly timeline?: TimelineInput;
  readonly lumenSchemaVersion?: string;
  readonly schemaVersion?: string;
}

export type NodeInput<TKind extends NodeKind = NodeKind> = NodeInputByKind[TKind];

type NodeInputByKind = {
  readonly [TKind in NodeKind]: WithExpressionParams<CompositionNodeInput<TKind>>;
};

/** Lets each param that takes `=...` strings take an `Expr` of the matching type too. */
type WithExpressionParams<TNode extends { readonly params?: object }> = Omit<TNode, "params"> & {
  readonly params?: {
    readonly [K in keyof NonNullable<TNode["params"]>]: ParamInput<NonNullable<TNode["params"]>[K]>;
  };
};

type ParamInput<TParam> = [Extract<TParam, ExpressionString>] extends [never]
  ? TParam
  : TParam | ParamExpr<Exclude<TParam, ExpressionString | undefined>>;

type ParamExpr<T> = [T] extends [boolean]
  ? never
  : [T] extends [number]
    ? Expr<number>
    : [T] extends [Vec2]
      ? Expr<Vec2>
      : [T] extends [string]
        ? Expr<string>
        : Expr<Color>;

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

export interface AudioTimelineInput {
  readonly clips: readonly AudioClipInput[];
  readonly tracks: readonly AudioTrackInput[];
  readonly [key: string]: unknown;
}

export type AudioTrackOptions = AudioTrackInput;

export interface AudioTrackInput {
  readonly id: string;
  readonly muted?: boolean;
  readonly name?: string;
  readonly solo?: boolean;
  readonly volume?: number;
  readonly [key: string]: unknown;
}

export type AudioClipOptions = Omit<
  AudioClipInput,
  | "duration_frames"
  | "duration_ms"
  | "id"
  | "source_id"
  | "source_start_ms"
  | "source_start_seconds"
  | "start_frame"
  | "start_ms"
  | "track_id"
> & {
  readonly durationMs?: number;
  readonly durationSeconds?: number;
  readonly id?: string;
  readonly sourceId: string;
  readonly sourceStartMs?: number;
  readonly sourceStartSeconds?: number;
  readonly startMs?: number;
  readonly startSeconds?: number;
};

export interface AudioClipInput {
  readonly duration_frames?: number;
  readonly duration_ms?: number;
  readonly id: string;
  readonly name?: string;
  readonly source_id: string;
  readonly source_start_ms?: number;
  readonly source_start_seconds?: number;
  readonly start_frame?: number;
  readonly start_ms?: number;
  readonly track_id: string;
  readonly volume?: number;
  readonly [key: string]: unknown;
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
