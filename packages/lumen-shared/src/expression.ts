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

import { quote } from "./quote.js";

declare const valueType: unique symbol;

/** A type an expression can evaluate to. */
export type Value = number | Vec2 | Color | string;

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

/**
 * An expression that evaluates to `T`. Arithmetic follows the evaluator: numbers combine freely,
 * vec2 values add and subtract with each other and scale by numbers or vec2 values, and colours
 * have no arithmetic.
 */
export class Expr<T extends Value = Value> {
  declare readonly [valueType]: T;
  readonly #source: string;

  constructor(source: string) {
    this.#source = source;
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
  return new Expr<T>(`(${lower(lhs)} ${operator} ${lower(rhs)})`);
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

/** The shortest decimal or exponent form that round-trips. */
function formatNumber(value: number): string {
  if (!Number.isFinite(value)) {
    throw new RangeError(`Expressions cannot contain \`${value}\`.`);
  }
  return String(value);
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
  return new Expr(`${name}(${(args as readonly AnyArg[]).map(lower).join(", ")})`);
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
  noise,
  outBack,
  outCubic,
  outExpo,
  outQuad,
  pow,
  progress,
  random,
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
export const tween = builtins.tween as Tween;

/**
 * `glyph`, `word`, `line` and `span` index the glyph a text node is drawing, and the counts
 * number them. They are only valid in the text node's `glyph_*` params.
 */
export const {
  fps,
  frame,
  glyph,
  glyphCount,
  height,
  line,
  lineCount,
  span,
  spanCount,
  time,
  width,
  word,
  wordCount,
} = Object.fromEntries(
  expressionGlobals.map((global) => [camelCase(global.name), new Expr(global.name)]),
) as { readonly [N in ExpressionGlobalName as CamelCase<N>]: Expr<number> };

/** Easing names for `tween`, such as `ease.outExpo` for `'out_expo'`. */
export const ease = Object.fromEntries(easingNames.map((name) => [camelCase(name), name])) as {
  readonly [N in EasingName as CamelCase<N>]: N;
};

/** `seconds` as a `1.5s` literal, which the evaluator converts to frames. */
export function sec(seconds: number): Expr<number> {
  return new Expr(`${formatNumber(seconds)}s`);
}

/** `milliseconds` as a `300ms` literal, which the evaluator converts to frames. */
export function ms(milliseconds: number): Expr<number> {
  return new Expr(`${formatNumber(milliseconds)}ms`);
}

/** A `#rrggbb` or `#rrggbbaa` colour literal. */
export function hex(color: `#${string}`): Expr<Color> {
  if (!/^#(?:[\da-f]{6}|[\da-f]{8})$/i.test(color)) {
    throw new TypeError(`Invalid colour \`${color}\`; use #rrggbb or #rrggbbaa.`);
  }
  return new Expr(color);
}

/**
 * Text an expression reads as itself. A string starting with `=` is an expression, so it is
 * escaped as `==`.
 */
export function literal(text: string): string {
  return text.startsWith("=") ? `=${text}` : text;
}

/**
 * A raw expression, such as `expr("if(frame > 30, 1, 0)")` or `expr("node(3, 'opacity')")`, for
 * forms the helpers don't cover. `source` has no leading `=`. It takes the type of the param it
 * is in, or the one you give, as in `expr<number>(..)`. Given neither, as in
 * `const x = expr("frame")`, it fits any param, and only the engine checks its type.
 */
export function expr<T extends Value = never>(source: string): Expr<T> {
  if (source.startsWith("=")) {
    throw new TypeError("`expr` takes the expression without its leading `=`.");
  }
  return new Expr<T>(source);
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

/**
 * Defines a composition function such as `rise(start)`. The implementation runs once, with each
 * parameter bound to its name, to build the body. Register it with `Composition.addFunction`.
 */
export function fn<A extends readonly AnyArg[], R extends Value>(
  signature: CheckedSignature<A>,
  impl: (...params: A) => Expr<R>,
): ExprFunction<A, R> {
  const match = /^\s*(\w+)\s*\((.*)\)\s*$/.exec(signature);
  if (match === null) {
    throw new TypeError(`Invalid function signature \`${signature}\`; expected \`name(a, b)\`.`);
  }
  const [, name = "", list = ""] = match;
  const params = list.trim() === "" ? [] : list.split(",").map((param) => param.trim());
  if (impl.length !== params.length) {
    throw new TypeError(
      `\`${name}\` declares ${params.length} parameters, but its implementation takes ${impl.length}.`,
    );
  }
  const body = impl(...(params.map((param) => new Expr(param)) as unknown as A));
  const callable = (...args: A) => new Expr<R>(`${name}(${args.map(lower).join(", ")})`);
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

/** `base` plus `by * tween(0, 1, ..)` per move, so moves can overlap. */
export function moves(base: Num, steps: readonly Move<number>[]): Expr<number>;
export function moves(base: Arg<Vec2>, steps: readonly Move<Vec2>[]): Expr<Vec2>;
export function moves(
  base: Num | Arg<Vec2>,
  steps: readonly Move<number>[] | readonly Move<Vec2>[],
): Expr {
  const initial = base instanceof Expr ? base : new Expr(lower(base));
  const untypedTween = tween as (...args: readonly AnyArg[]) => Expr;
  return steps.reduce<Expr>((sum, { by, start, duration, ease }) => {
    const eased = untypedTween(0, 1, start, duration, ...(ease === undefined ? [] : [ease]));
    return binary(sum, "+", binary(by, "*", eased));
  }, initial);
}
