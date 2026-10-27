import {
  easingNames,
  expressionBuiltins,
  expressionGlobals,
  type Color,
  type EasingName,
  type ExpressionGlobalName,
  type ExpressionString,
  type Vec2,
} from "@lumiscia/lumen-types";

declare const valueType: unique symbol;

/** A type an expression can evaluate to. */
export type Value = number | Vec2 | Color | string;

export type Kind = "number" | "vec2" | "color" | "string";

export type Num = number | Expr<number>;

/** An expression, or a literal of the same type. Tuples lower to `vec2(..)`, `rgb(..)` or `rgba(..)`. */
export type Arg<T extends Value> =
  | Expr<T>
  | ([T] extends [number]
      ? number
      : [T] extends [Vec2]
        ? readonly [Num, Num]
        : [T] extends [Color]
          ? readonly [Num, Num, Num] | readonly [Num, Num, Num, Num]
          : [T] extends [string]
            ? string
            : never);

export type AnyArg = Expr | Arg<number> | Arg<Vec2> | Arg<Color> | Arg<string>;

let exprKind: (expr: Expr) => Kind | undefined;

/**
 * An expression that evaluates to `T`. Arithmetic follows the evaluator: numbers combine freely,
 * vec2 values add and subtract with each other and scale by numbers or vec2 values, and colours
 * have no arithmetic.
 */
export class Expr<T extends Value = Value> {
  declare readonly [valueType]: T;
  readonly #source: string;
  readonly #kind: Kind | undefined;

  static {
    exprKind = (expr) => expr.#kind;
  }

  constructor(source: string, kind?: Kind) {
    this.#source = source;
    this.#kind = kind;
  }

  add(this: Expr<number>, other: Num): Expr<number>;
  add(this: Expr<Vec2>, other: Arg<Vec2>): Expr<Vec2>;
  add(other: AnyArg): Expr {
    return binary(this, "+", other);
  }

  sub(this: Expr<number>, other: Num): Expr<number>;
  sub(this: Expr<Vec2>, other: Arg<Vec2>): Expr<Vec2>;
  sub(other: AnyArg): Expr {
    return binary(this, "-", other);
  }

  mul(this: Expr<number>, other: Num): Expr<number>;
  mul(this: Expr<number>, other: Arg<Vec2>): Expr<Vec2>;
  mul(this: Expr<Vec2>, other: Num | Arg<Vec2>): Expr<Vec2>;
  mul(other: AnyArg): Expr {
    return binary(this, "*", other);
  }

  div(this: Expr<number>, other: Num): Expr<number>;
  div(this: Expr<number>, other: Arg<Vec2>): Expr<Vec2>;
  div(this: Expr<Vec2>, other: Num | Arg<Vec2>): Expr<Vec2>;
  div(other: AnyArg): Expr {
    return binary(this, "/", other);
  }

  oneMinus(this: Expr<number>): Expr<number> {
    return binary(1, "-", this);
  }

  clamp(this: Expr<number>, min: Num, max: Num): Expr<number> {
    return clamp(this, min, max);
  }

  toString(): ExpressionString {
    return `=${this.#source}`;
  }

  toJSON(): ExpressionString {
    return this.toString();
  }
}

function binary<T extends Value>(lhs: AnyArg, operator: string, rhs: AnyArg): Expr<T> {
  const kinds = [kindOf(lhs), kindOf(rhs)];
  const kind = kinds.includes("vec2")
    ? "vec2"
    : kinds.every((kind) => kind === "number")
      ? "number"
      : undefined;
  return new Expr<T>(`(${lower(lhs)} ${operator} ${lower(rhs)})`, kind);
}

function kindOf(value: AnyArg): Kind | undefined {
  if (value instanceof Expr) {
    return exprKind(value);
  }
  if (typeof value === "number") {
    return "number";
  }
  if (typeof value === "string") {
    return "string";
  }
  return value.length === 2 ? "vec2" : "color";
}

const tupleBuiltins: Readonly<Record<number, string>> = { 2: "vec2", 3: "rgb", 4: "rgba" };

function lower(value: AnyArg): string {
  if (value instanceof Expr) {
    return value.toString().slice(1);
  }
  if (typeof value === "number") {
    return formatNumber(value);
  }
  if (typeof value === "string") {
    return quote(value);
  }
  const builtin = tupleBuiltins[value.length];
  if (builtin === undefined) {
    throw new TypeError(`Cannot use a ${value.length}-element array as an expression value.`);
  }
  return `${builtin}(${value.map(lower).join(", ")})`;
}

/** The shortest decimal that round-trips, without the exponents the expression lexer rejects. */
function formatNumber(value: number): string {
  if (!Number.isFinite(value)) {
    throw new RangeError(`Expressions cannot contain \`${value}\`.`);
  }
  if (value === 0) {
    return "0";
  }
  const text = String(Math.abs(value));
  const sign = value < 0 ? "-" : "";
  const [mantissa = text, exponent] = text.split("e");
  if (exponent === undefined) {
    return sign + text;
  }
  const digits = mantissa.replace(".", "");
  const point = 1 + Number(exponent);
  if (point <= 0) {
    return `${sign}0.${"0".repeat(-point)}${digits}`;
  }
  return sign + digits.padEnd(point, "0");
}

const escapes: Readonly<Record<string, string>> = {
  "\\": "\\\\",
  "'": "\\'",
  "\n": "\\n",
  "\r": "\\r",
  "\t": "\\t",
};

function quote(text: string): string {
  return `'${text.replace(/[\\'\n\r\t]/g, (char) => escapes[char] ?? char)}'`;
}

export type CamelCase<S extends string> = S extends `${infer Head}_${infer Tail}`
  ? `${Head}${Capitalize<CamelCase<Tail>>}`
  : S;

function camelCase(name: string): string {
  return name.replace(/_([a-z])/g, (_, char: string) => char.toUpperCase());
}

type BuiltinSpec = (typeof expressionBuiltins)[number];
type SpecArg = BuiltinSpec["args"][number];

type KindArg<K> = K extends "number"
  ? Num
  : K extends "vec2"
    ? Arg<Vec2>
    : K extends "color"
      ? Arg<Color>
      : K extends "string"
        ? Arg<string>
        : K extends "easing"
          ? EasingName
          : AnyArg;

type KindValue<K> = K extends "number"
  ? number
  : K extends "vec2"
    ? Vec2
    : K extends "color"
      ? Color
      : K extends "string"
        ? string
        : Value;

/** Skipped arguments must be `undefined` through to the end, so an interior hole is a type error. */
type SkippedArgs<A> = A extends readonly [SpecArg, ...infer Tail]
  ? [] | [undefined, ...SkippedArgs<Tail>]
  : [];

type OptionalArgs<A> = A extends readonly [infer Head extends SpecArg, ...infer Tail]
  ?
      | [(KindArg<Head["kind"]> | undefined)?]
      | [KindArg<Head["kind"]>, ...OptionalArgs<Tail>]
      | [undefined, ...SkippedArgs<Tail>]
  : [];

type FixedArgs<A, Min, Done extends unknown[] = []> = Done["length"] extends Min
  ? [...Done, ...OptionalArgs<A>]
  : A extends readonly [infer Head extends SpecArg, ...infer Tail]
    ? FixedArgs<Tail, Min, [...Done, KindArg<Head["kind"]>]>
    : Done;

type Repeat<T, Min, Done extends unknown[]> = Done["length"] extends Min
  ? [...Done, ...T[]]
  : Repeat<T, Min, [...Done, T]>;

/** A `null` `maxArgs` repeats the last argument. */
type VariadicArgs<A, Min, Done extends unknown[] = []> = A extends readonly [
  infer Last extends SpecArg,
]
  ? Repeat<KindArg<Last["kind"]>, Min, Done>
  : A extends readonly [infer Head extends SpecArg, ...infer Tail]
    ? VariadicArgs<Tail, Min, [...Done, KindArg<Head["kind"]>]>
    : Done;

type BuiltinArgs<S extends BuiltinSpec> = S["maxArgs"] extends null
  ? VariadicArgs<S["args"], S["minArgs"]>
  : FixedArgs<S["args"], S["minArgs"]>;

/** One helper per entry in `expressionBuiltins`, named in camelCase. */
export type Builtins = {
  readonly [S in BuiltinSpec as CamelCase<S["name"]>]: (
    ...args: BuiltinArgs<S>
  ) => Expr<KindValue<S["returns"]>>;
};

function call(spec: BuiltinSpec, given: readonly (AnyArg | undefined)[]): Expr {
  const { maxArgs, minArgs, name } = spec;
  let end = given.length;
  while (end > 0 && given[end - 1] === undefined) {
    end -= 1;
  }
  const args = given.slice(0, end);
  if (args.includes(undefined)) {
    throw new TypeError(`\`${name}\` cannot skip an optional argument before a later one.`);
  }
  if (args.length < minArgs || (maxArgs !== null && args.length > maxArgs)) {
    const expected =
      maxArgs === null
        ? `at least ${minArgs}`
        : maxArgs === minArgs
          ? minArgs
          : `${minArgs} to ${maxArgs}`;
    throw new TypeError(`\`${name}\` expects ${expected} arguments, got ${args.length}.`);
  }
  const [first] = args as readonly AnyArg[];
  const kind =
    spec.returns !== "any" ? spec.returns : first === undefined ? undefined : kindOf(first);
  return new Expr(`${name}(${(args as readonly AnyArg[]).map(lower).join(", ")})`, kind);
}

const builtins = Object.fromEntries(
  expressionBuiltins.map((spec) => [
    camelCase(spec.name),
    (...args: (AnyArg | undefined)[]) => call(spec, args),
  ]),
) as unknown as Builtins;

export interface Interpolate {
  (from: Num, to: Num, t: Num): Expr<number>;
  (from: Arg<Vec2>, to: Arg<Vec2>, t: Num): Expr<Vec2>;
  (from: Arg<Color>, to: Arg<Color>, t: Num): Expr<Color>;
}

export interface Tween {
  (from: Num, to: Num, start: Num, duration: Num, ease?: EasingName): Expr<number>;
  (from: Arg<Vec2>, to: Arg<Vec2>, start: Num, duration: Num, ease?: EasingName): Expr<Vec2>;
  (from: Arg<Color>, to: Arg<Color>, start: Num, duration: Num, ease?: EasingName): Expr<Color>;
}

export const {
  abs,
  ceil,
  clamp,
  cos,
  cubicBezier,
  floor,
  fract,
  inBack,
  inCubic,
  inExpo,
  inOutBack,
  inOutCubic,
  inOutExpo,
  inOutQuad,
  inQuad,
  lowercase,
  max,
  min,
  mod,
  outBack,
  outCubic,
  outExpo,
  outQuad,
  pow,
  progress,
  rgb,
  rgba,
  round,
  sin,
  smoothstep,
  spring,
  step,
  textHeight,
  textWidth,
  uppercase,
  vec2,
} = builtins;
// `from` and `to` share a type, which the table can only describe as `any`.
export const lerp = builtins.lerp as Interpolate;
export const linear = builtins.linear as Interpolate;
export const tween = builtins.tween as Tween;

/**
 * `glyph`, `word` and `line` index the glyph a text node is drawing, and the counts number
 * them. They are only valid in the text node's `glyph_*` params.
 */
export const {
  fps,
  frame,
  glyph,
  glyphCount,
  height,
  line,
  lineCount,
  time,
  width,
  word,
  wordCount,
} = Object.fromEntries(
  expressionGlobals.map((global) => [camelCase(global.name), new Expr(global.name, "number")]),
) as { readonly [N in ExpressionGlobalName as CamelCase<N>]: Expr<number> };

/** Easing names for `tween`, such as `ease.outExpo` for `'out_expo'`. */
export const ease = Object.fromEntries(easingNames.map((name) => [camelCase(name), name])) as {
  readonly [N in EasingName as CamelCase<N>]: N;
};

/** `seconds` as a `1.5s` literal, which the evaluator converts to frames. */
export function sec(seconds: number): Expr<number> {
  return new Expr(`${formatNumber(seconds)}s`, "number");
}

/** `milliseconds` as a `300ms` literal, which the evaluator converts to frames. */
export function ms(milliseconds: number): Expr<number> {
  return new Expr(`${formatNumber(milliseconds)}ms`, "number");
}

/** A `#rrggbb` or `#rrggbbaa` colour literal. */
export function hex(color: `#${string}`): Expr<Color> {
  if (!/^#(?:[\da-f]{6}|[\da-f]{8})$/i.test(color)) {
    throw new TypeError(`Invalid colour \`${color}\`; use #rrggbb or #rrggbbaa.`);
  }
  return new Expr(color, "color");
}

/** Reads a template prop. Inside `defineComposition`, `build` gets typed reads of its props. */
export function readProp<T extends Value = Value>(name: string, kind?: Kind): Expr<T> {
  return new Expr(`prop(${quote(name)})`, kind);
}

export type FunctionSignature = `${string}(${string})`;

/** An entry of a composition's `functions`. */
export interface FunctionDefinition {
  readonly signature: FunctionSignature;
  readonly body: string;
}

export interface ExprFunction<
  A extends readonly AnyArg[],
  R extends Value,
> extends FunctionDefinition {
  (...args: A): Expr<R>;
}

/** Rejects implementations whose parameters can't hold the expressions `fn` passes them. */
type CheckedSignature<A extends readonly unknown[]> = false extends {
  [I in keyof A]: Expr<never> extends A[I] ? true : false;
}[number]
  ? "fn parameters must accept expressions, such as `Num`"
  : FunctionSignature;

const identifier = /^[A-Za-z_]\w*$/;
const reservedNames: ReadonlySet<string> = new Set([
  "if",
  "node",
  "true",
  "false",
  ...expressionBuiltins.map(({ name }) => name),
  ...expressionGlobals.map(({ name }) => name),
]);

/**
 * Defines a composition function such as `rise(start)`. The implementation runs once, with each
 * parameter bound to its name, to build the body. Register it with `Composition.addFunction`.
 */
export function fn<A extends readonly AnyArg[], R extends Value>(
  signature: CheckedSignature<A>,
  impl: (...params: A) => Expr<R>,
): ExprFunction<A, R> {
  const match = /^\s*(\w+)\s*\((.*)\)\s*$/.exec(signature);
  const [, name = "", list = ""] = match ?? [];
  const params = list.trim() === "" ? [] : list.split(",").map((param) => param.trim());
  if (!identifier.test(name) || !params.every((param) => identifier.test(param))) {
    throw new TypeError(`Invalid function signature \`${signature}\`; expected \`name(a, b)\`.`);
  }
  const reserved = [name, ...params].find((identifier) => reservedNames.has(identifier));
  if (reserved !== undefined) {
    throw new TypeError(`\`${reserved}\` is a reserved name.`);
  }
  if (params.includes(name)) {
    throw new TypeError(`Parameter \`${name}\` has the same name as its function.`);
  }
  const repeated = params.find((param, index) => params.indexOf(param) !== index);
  if (repeated !== undefined) {
    throw new TypeError(`Parameter \`${repeated}\` is declared twice.`);
  }
  if (impl.length !== params.length) {
    throw new TypeError(
      `\`${name}\` declares ${params.length} parameters, but its implementation takes ${impl.length}.`,
    );
  }
  const body = impl(...(params.map((param) => new Expr(param)) as unknown as A));
  const kind = exprKind(body);
  const callable = (...args: A) => new Expr<R>(`${name}(${args.map(lower).join(", ")})`, kind);
  return Object.assign(callable, {
    signature: `${name}(${params.join(", ")})` as const,
    body: body.toString().slice(1),
  });
}

export interface Move<T extends number | Vec2> {
  readonly by: Arg<T>;
  readonly start: Num;
  readonly duration: Num;
  readonly ease?: EasingName;
}

function zeroOf(kind: Kind | undefined): Num | Arg<Vec2> {
  if (kind === "number") {
    return 0;
  }
  if (kind === "vec2") {
    return vec2(0, 0);
  }
  throw new TypeError(
    "`moves` cannot tell whether its values are numbers or vec2s; type the base.",
  );
}

/** `base` plus one `tween` from zero to `by` per move, so moves can overlap. */
export function moves(base: Num, steps: readonly Move<number>[]): Expr<number>;
export function moves(base: Arg<Vec2>, steps: readonly Move<Vec2>[]): Expr<Vec2>;
export function moves(
  base: Num | Arg<Vec2>,
  steps: readonly Move<number>[] | readonly Move<Vec2>[],
): Expr {
  const kind = kindOf(base);
  if (kind === "color" || kind === "string") {
    throw new TypeError("`moves` needs a number or vec2 base.");
  }
  const initial = base instanceof Expr ? base : new Expr(lower(base), kind);
  if (steps.length === 0) {
    return initial;
  }
  const zero = zeroOf(kind ?? steps.map(({ by }) => kindOf(by)).find((kind) => kind !== undefined));
  const untypedTween = tween as (...args: readonly AnyArg[]) => Expr;
  return steps.reduce<Expr>((sum, { by, start, duration, ease }) => {
    const args = [zero, by, start, duration, ...(ease === undefined ? [] : [ease])];
    return binary(sum, "+", untypedTween(...args));
  }, initial);
}
