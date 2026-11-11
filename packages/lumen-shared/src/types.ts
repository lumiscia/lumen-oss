import type {
  Color,
  CompositionNodeInput,
  CompositionPropDeclaration,
  CompositionTemplate,
  ExpressionString,
  HexColor,
  LumenComposition,
  NodeInputPortKind,
  NodeInputPortName,
  NodeKind,
  NodeOutputKind,
  NodePortKind,
  NodeVariadicInputPortName,
  TemplateRenderSettings,
  TextContent,
  Vec2,
  Vec3,
  nodeInputPorts,
} from "@lumiscia/lumen-types";

import type { Composition } from "./composition.js";
import type { Expr, Vec3Value } from "./expression.js";

export interface CompositionOptions {
  readonly audio?: LumenComposition["audio"];
  readonly metadata?: LumenComposition["metadata"];
  readonly renderSettings?: RenderSettingsInput;
  readonly timeline?: TimelineInput;
  readonly lumenSchemaVersion?: LumenComposition["lumenSchemaVersion"];
}

export type NodeInput<TKind extends NodeKind = NodeKind> = NodeInputByKind[TKind];

type NodeInputByKind = {
  readonly [TKind in NodeKind]: WithExpressionParams<CompositionNodeInput<TKind>> & {
    /** The nodes that feed the ports, connected in order. */
    readonly inputs?: NodeInputs<TKind>;
  };
};

/**
 * A port takes one node that outputs what the port takes; a variadic port such as `layers` takes
 * an array.
 */
type NodeInputs<TKind extends NodeKind> = [NodeInputPortName<TKind>] extends [never]
  ? undefined
  : NodePorts<TKind>;

type NodePorts<TKind extends NodeKind> = {
  readonly [TPort in NodeInputPortName<TKind>]?: TPort extends NodeVariadicInputPortName<TKind>
    ?
        | NodeSource<NodeInputPortKind<TKind, TPort>>
        | readonly NodeSource<NodeInputPortKind<TKind, TPort>>[]
    : NodeSource<NodeInputPortKind<TKind, TPort>>;
};

/** A node value whose `type` outputs `TOutput`, as `nodes` builds it. */
export type NodeValue<TOutput extends NodePortKind = NodePortKind> = NodeInput<
  NodeKindOutputting<TOutput>
>;

/**
 * What a port reads: a node of the composition, or a node value, which becomes a node of the
 * composition when the node reading it is added. One value is one node, however many read it.
 */
export type NodeSource<TOutput extends NodePortKind = NodePortKind> =
  | NodeReference<TOutput>
  | NodeValue<TOutput>;

/** A node's params and input ports by name in one object, as `nodes` takes them. */
export type NodeProps<TKind extends NodeKind> = NodeParamProps<TKind> &
  ([NodeInputPortName<TKind>] extends [never] ? unknown : NodePorts<TKind>);

/** A kind without params has an open `Record<string, never>` there, which would take no ports. */
type NodeParamProps<TKind extends NodeKind> =
  NonNullable<NodeInput<TKind>["params"]> extends infer TParams
    ? string extends keyof TParams
      ? unknown
      : TParams
    : never;

/**
 * One function per node kind, named after its `type`. Each takes the node's params and input
 * ports in one object and returns the node value, without adding anything.
 */
export type NodeFunctions = {
  readonly [TKind in keyof typeof nodeInputPorts]: (
    ...props: Record<never, never> extends NodeProps<TKind>
      ? [props?: NodeProps<TKind>]
      : [props: NodeProps<TKind>]
  ) => NodeInput<TKind>;
};

/** Lets each param that takes `=...` strings take an `Expr` of the matching type too. */
type WithExpressionParams<TNode extends { readonly params?: object }> = Omit<TNode, "params"> & {
  readonly params?: {
    readonly [K in keyof NonNullable<TNode["params"]>]: ExpressionInput<
      NonNullable<TNode["params"]>[K]
    >;
  };
};

/**
 * `T`, where every field that takes `=...` strings, at any depth of objects and arrays, also
 * takes an `Expr` of the matching type. Use it to type a value built outside `addNode`:
 * `ExpressionInput<TextSpan>`.
 */
export type ExpressionInput<T> =
  | ([Extract<T, ExpressionString>] extends [never]
      ? never
      : ParamExpr<Exclude<T, ExpressionString | undefined>>)
  | NestedInput<T>;

type NestedInput<T> = T extends ExpressionString
  ? T
  : T extends object
    ? { [K in keyof T]: ExpressionInput<T[K]> }
    : T;

/** The `Expr` a param of type `T` takes. A vec3 param takes `vec3(..)`, never a colour. */
type ParamExpr<T> = [T] extends [boolean]
  ? never
  : [T] extends [number]
    ? Expr<number>
    : [T] extends [Vec2]
      ? Expr<Vec2>
      : [T] extends [Vec3]
        ? Expr<Vec3Value>
        : [T] extends [string]
          ? Expr<string>
          : [T] extends [TextContent]
            ? Expr<string>
            : [T] extends [Color]
              ? Expr<Color>
              : MemberExpr<T>;

/** The `Expr`s a union such as `Paint` or `ShaderUniformValue` takes, one per kind of member. */
type MemberExpr<T> = T extends number
  ? Expr<number>
  : T extends readonly [unknown, unknown]
    ? Expr<Vec2>
    : T extends readonly unknown[] | `#${string}`
      ? Expr<Color>
      : T extends string
        ? Expr<string>
        : never;

/** The node kinds that output `TOutput`. */
type NodeKindOutputting<TOutput extends NodePortKind> = {
  readonly [TKind in NodeKind]: NodeOutputKind<TKind> extends TOutput ? TKind : never;
}[NodeKind];

/**
 * A node of the composition, by id or as `addNode` returned it. With `TOutput`, it is one for a
 * port that takes that kind: a node whose `type` outputs another kind doesn't fit. A bare id isn't
 * checked.
 */
export type NodeReference<TOutput extends NodePortKind = NodePortKind> =
  | number
  | { readonly id: number; readonly type?: NodeKindOutputting<TOutput> };

/**
 * `toPort` is checked against the target's input ports, and the source against what that port
 * takes, when `connect` is given the node itself.
 */
export interface ConnectOptions<
  TKind extends NodeKind = NodeKind,
  TPort extends NodeInputPortName<TKind> = NodeInputPortName<TKind>,
> {
  readonly toPort: TPort;
}

export type Size = TemplateRenderSettings;

export type RenderSettingsInput = Partial<LumenComposition["render_settings"]>;

export type TimelineInput = Partial<LumenComposition["timeline"]> & {
  readonly durationSeconds?: number;
};

/** A track declared with `addAudioTrack`. Gain scales every clip on the track. */
export interface AudioTrackOptions {
  readonly id: string;
  readonly name?: string;
  readonly muted?: boolean;
  readonly solo?: boolean;
  readonly gain?: number;
}

/** A clip of an audio source, with every time in seconds. */
export interface AudioClipOptions {
  /** The audio source's id, as registered with the host. */
  readonly source: string;
  /** Where the clip starts on the timeline. */
  readonly start: number;
  readonly duration: number;
  /** Where in the source the clip starts playing. */
  readonly offset?: number;
  /** The track to play on, declared as a plain track if it isn't one yet. Defaults to `main`. */
  readonly trackId?: string;
  readonly gain?: number;
  readonly id?: string;
  readonly name?: string;
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
 * The values a field of `TType` accepts. A vec2 field takes a whole expression and a vec3 field a
 * vec3 `Expr`; other vectors take components. `vec3<f32>`/`vec4<f32>` also take a hex colour
 * (`#rrggbb` for vec3) or a colour `Expr`.
 */
export type WgslUniformValue<TType extends WgslUniformType> = TType extends `vec2<${string}>`
  ? readonly [WgslComponent, WgslComponent] | ExpressionString | Expr<Vec2>
  : TType extends `vec3<${infer TScalar}>`
    ?
        | readonly [WgslComponent, WgslComponent, WgslComponent]
        | Expr<Vec3Value>
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
export type PropType = CompositionPropDeclaration["type"];

/**
 * A template prop, declared with `prop.string(..)`, `prop.number(..)`, `prop.color(..)`,
 * `prop.vec2(..)` or `prop.vec3(..)`.
 */
export type PropDeclaration<TType extends PropType = PropType> = Extract<
  CompositionPropDeclaration,
  { readonly type: TType }
>;

type PropValueByType = {
  readonly [TType in PropType]: NonNullable<PropDeclaration<TType>["default"]>;
};

export type PropDeclarations = Readonly<
  Record<string, { [TType in PropType]: PropDeclaration<TType> }[PropType]>
>;

/** A typed `prop('name')` read for each declared prop. */
export type PropReads<TProps extends PropDeclarations> = {
  readonly [TName in keyof TProps]: ParamExpr<PropValueByType[TProps[TName]["type"]]>;
};

/** Prop values for a clip. Strings are literal text; pass an `Expr` for an expression. */
export type ClipPropInputs<TProps extends PropDeclarations> = [keyof TProps] extends [never]
  ? Readonly<Record<string, never>>
  : {
      readonly [TName in keyof TProps]?:
        | PropValueByType[TProps[TName]["type"]]
        | ParamExpr<PropValueByType[TProps[TName]["type"]]>;
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
  /**
   * Returns the node the template outputs: a node value, or a node it added to `composition`.
   */
  readonly build: (composition: Composition, props: PropReads<TProps>) => NodeSource<"raster">;
}

/** Where a clip plays its template. `start` and `duration` count the parent's frames. */
export type ClipOptions<TProps extends PropDeclarations> = Omit<
  NonNullable<NodeInput<"clip">["params"]>,
  "composition" | "props"
> & {
  readonly id?: number;
  readonly props?: ClipPropInputs<TProps>;
};

export interface CompositionDefinition<TProps extends PropDeclarations = PropDeclarations> {
  readonly name: string;
  readonly template: CompositionTemplate;
  /** A `clip` node playing the template; adding it registers the template. */
  readonly clip: (options?: ClipOptions<TProps>) => NodeInput<"clip">;
}
