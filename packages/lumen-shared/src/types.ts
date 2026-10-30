import type {
  ClipEnd,
  Color,
  CompositionNode,
  CompositionNodeInput,
  CompositionTemplate,
  ExpressionString,
  HexColor,
  LumenComposition,
  NodeInputPortName,
  NodeKind,
  Vec2,
} from "@lumiscia/lumen-types";

import type { Composition } from "./composition.js";
import type { Expr, Num } from "./expression.js";

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

/** `toPort` is checked against the target's input ports when `connect` is given the node itself. */
export interface ConnectOptions<TKind extends NodeKind = NodeKind> {
  readonly fromPort?: string;
  readonly toPort: NodeInputPortName<TKind>;
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

export type WgslUniformType =
  | "f32"
  | "i32"
  | "u32"
  | "vec2<f32>"
  | "vec2<i32>"
  | "vec2<u32>"
  | "vec3<f32>"
  | "vec3<i32>"
  | "vec3<u32>"
  | "vec4<f32>"
  | "vec4<i32>"
  | "vec4<u32>";

/** Uniform field names mapped to their WGSL types, in struct order. */
export type WgslUniforms = Readonly<Record<string, WgslUniformType>>;

type WgslComponent = number | ExpressionString | Expr<number>;

/**
 * The values a field of `TType` accepts. Vectors other than vec2 take components rather than a
 * whole expression, and `vec3<f32>`/`vec4<f32>` also take a hex colour (`#rrggbb` for vec3) or a
 * colour `Expr`.
 */
export type WgslUniformValue<TType extends WgslUniformType> = TType extends `vec2<${string}>`
  ? readonly [WgslComponent, WgslComponent] | ExpressionString | Expr<Vec2>
  : TType extends `vec3<${infer TScalar}>`
    ?
        | readonly [WgslComponent, WgslComponent, WgslComponent]
        | (TScalar extends "f32" ? HexColor | Expr<Color> : never)
    : TType extends `vec4<${infer TScalar}>`
      ?
          | readonly [WgslComponent, WgslComponent, WgslComponent, WgslComponent]
          | (TScalar extends "f32" ? HexColor | Expr<Color> : never)
      : WgslComponent;

export type WgslUniformValues<TUniforms extends WgslUniforms> = [keyof TUniforms] extends [never]
  ? Readonly<Record<string, never>>
  : { readonly [TName in keyof TUniforms]?: WgslUniformValue<TUniforms[TName]> };

export interface WgslShaderOptions<TUniforms extends WgslUniforms> {
  readonly uniforms?: TUniforms;
  /** WGSL source that reads `uniforms.<name>`; the struct and its binding are appended. */
  readonly shader: string;
}

export interface WgslShader<TUniforms extends WgslUniforms> {
  /** The full shader source: `shader`, then the generated `Uniforms` struct and binding. */
  readonly source: string;
  readonly node: (values?: WgslUniformValues<TUniforms>) => NodeInput<"wgsl_shader">;
}

/** The value types a template prop can declare. */
export type PropType = "string" | "number" | "color" | "vec2";

interface PropValueByType {
  readonly string: string;
  readonly number: number;
  readonly color: Color;
  readonly vec2: Vec2;
}

/** A template prop, declared with `prop.string(..)`, `prop.number(..)`, `prop.color(..)` or `prop.vec2(..)`. */
export interface PropDeclaration<TType extends PropType = PropType> {
  readonly type: TType;
  readonly default?: PropValueByType[TType];
}

export type PropDeclarations = Readonly<
  Record<string, { [TType in PropType]: PropDeclaration<TType> }[PropType]>
>;

/** A typed `prop('name')` read for each declared prop. */
export type PropReads<TProps extends PropDeclarations> = {
  readonly [TName in keyof TProps]: Expr<PropValueByType[TProps[TName]["type"]]>;
};

/** Prop values for a clip. Strings are literal text; pass an `Expr` for an expression. */
export type ClipPropInputs<TProps extends PropDeclarations> = [keyof TProps] extends [never]
  ? Readonly<Record<string, never>>
  : {
      readonly [TName in keyof TProps]?:
        | PropValueByType[TProps[TName]["type"]]
        | Expr<PropValueByType[TProps[TName]["type"]]>;
    };

export interface CompositionDefinitionOptions<TProps extends PropDeclarations> {
  /** The template's key in `compositions`. */
  readonly name: string;
  /** Defaults to the size of the composition the template is used in. */
  readonly size?: Size;
  /** Defaults to the fps of the composition the template is used in. Needs a `duration`. */
  readonly fps?: number;
  /** In frames. Defaults to the timeline of the composition the template is used in. */
  readonly duration?: number;
  readonly props?: TProps;
  /** Adds the template's nodes and returns the one it outputs. */
  readonly build: (composition: Composition, props: PropReads<TProps>) => NodeReference;
}

/** Where a clip plays its template. `start` and `duration` count the parent's frames. */
export interface ClipOptions<TProps extends PropDeclarations> {
  readonly id?: number;
  readonly start?: Num;
  /** Defaults to the rest of the template. */
  readonly duration?: Num;
  /** Template frames skipped at the start. */
  readonly offset?: Num;
  readonly speed?: Num;
  readonly end?: ClipEnd;
  readonly props?: ClipPropInputs<TProps>;
}

export interface CompositionDefinition<TProps extends PropDeclarations = PropDeclarations> {
  readonly name: string;
  readonly template: CompositionTemplate;
  /** The built template. Compositions that add its clips also take its functions and nested templates. */
  readonly composition: Composition;
  /** A `clip` node playing the template; adding it registers the template. */
  readonly clip: (options?: ClipOptions<TProps>) => NodeInput<"clip">;
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
