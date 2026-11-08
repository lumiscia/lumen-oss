import type {
  ClipProps,
  DashPattern,
  FontFeatures,
  GradientPaint,
  NodeCategory,
  NodeKind,
  NodeParamKind,
  NodePortKind,
  ShaderUniforms,
  TextContent,
  TextHighlight,
  TextLine,
  TextStroke,
} from "./generated/meta.js";

/** What the schema says about one node kind: its ports, params and defaults. */
export interface NodeSpec {
  readonly kind: NodeKind;
  readonly name: string;
  readonly description: string;
  readonly category: NodeCategory;
  readonly inputs: readonly NodeInputPortSpec[];
  readonly outputs: readonly NodeOutputPortSpec[];
  readonly params: readonly NodeParamSpec[];
  readonly defaultParams: Readonly<Record<string, NodeLiteralValue>>;
}

export interface NodeInputPortSpec {
  readonly name: string;
  readonly kind: NodePortKind;
  readonly optional: boolean;
  readonly variadic: boolean;
}

export interface NodeOutputPortSpec {
  readonly name: string;
  readonly kind: NodePortKind;
}

export interface NodeParamSpec {
  readonly id: string;
  readonly name: string;
  readonly kind: NodeParamKind;
  readonly description: string;
  /** Absent for params that are unset unless given. */
  readonly defaultValue?: NodeLiteralValue;
  readonly enumOptions?: readonly NodeEnumOptionSpec[];
  readonly constraints?: ParamConstraintsSpec;
}

export interface ParamConstraintsSpec {
  readonly min?: number;
  readonly max?: number;
  readonly step?: number;
  readonly format?: string;
  readonly multiline?: boolean;
  readonly recommendedRows?: number;
  readonly role?: string;
}

export interface NodeEnumOptionSpec {
  readonly name: string;
  readonly label: string;
  readonly value: number;
}

export type NodeLiteralValue =
  | boolean
  | number
  | string
  | GradientPaint
  | ShaderUniforms
  | ClipProps
  | TextContent
  | TextLine
  | TextHighlight
  | FontFeatures
  | TextStroke
  | DashPattern
  | readonly [number, number]
  | readonly [number, number, number]
  | readonly [number, number, number, number]
  | readonly number[]
  | readonly string[];
