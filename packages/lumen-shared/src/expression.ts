import { easingNames, type Color, type EasingName, type Vec2 } from "@lumiscia/lumen-types";

import {
  Expr,
  binary,
  formatNumber,
  lower,
  operand,
  rawExpressions,
  type AnyArg,
  type Arg,
  type Condition,
  type EasingArg,
  type Num,
  type Value,
  type Vec3Value,
} from "./expression-core.js";
import { ease as applyEasing, tween } from "./generated/expressions.js";
import { quote } from "./quote.js";
import type { NodeReference } from "./types.js";

export * from "./generated/expressions.js";
export type {
  AnyArg,
  Arg,
  Condition,
  Easing,
  EasingArg,
  Key,
  KeyValue,
  Num,
  Value,
  Vec3Value,
} from "./expression-core.js";
export { Expr } from "./expression-core.js";

type CamelCase<S extends string> = S extends `${infer Head}_${infer Tail}`
  ? `${Head}${Capitalize<CamelCase<Tail>>}`
  : S;

function camelCase(name: string): string {
  return name.replace(/_([a-z])/g, (_, char: string) => char.toUpperCase());
}

/**
 * `ease(easing, t)` applies an easing to progress `t`. Its properties are the easing names, such
 * as `ease.outExpo` for `'out_expo'`.
 */
export const ease = Object.assign(
  applyEasing,
  Object.fromEntries(easingNames.map((name) => [camelCase(name), name])) as {
    readonly [N in EasingName as CamelCase<N>]: N;
  },
);

/**
 * `then` when `condition` is true, otherwise `otherwise`, which lower to `if(..)`. Only the chosen
 * branch is evaluated. The branches share a type, as in `when(frame.gte(30), hex("#ffffff"), [0, 0, 0])`.
 */
export function when(condition: Condition, then: Num, otherwise: Num): Expr<number>;
export function when(condition: Condition, then: Arg<Vec2>, otherwise: Arg<Vec2>): Expr<Vec2>;
export function when(
  condition: Condition,
  then: Arg<Vec3Value>,
  otherwise: Arg<Vec3Value>,
): Expr<Vec3Value>;
export function when(condition: Condition, then: Arg<Color>, otherwise: Arg<Color>): Expr<Color>;
export function when(condition: Condition, then: Arg<string>, otherwise: Arg<string>): Expr<string>;
export function when(condition: Condition, then: AnyArg, otherwise: AnyArg): Expr {
  return new Expr(`if(${lower(condition)}, ${lower(then)}, ${lower(otherwise)})`);
}

/**
 * The value of another node's param, as in `node(title, "font_size")`. The engine decides the
 * type when it reads the param, so it takes the type of the param it is in, or the one you give,
 * as in `node<number>(..)`, like `expr`.
 */
export function node<T extends Value = never>(id: NodeReference, property: string): Expr<T> {
  const value = typeof id === "number" ? id : id.id;
  if (!Number.isSafeInteger(value) || value < 0) {
    throw new RangeError(`\`node\` expects an unsigned integer id, got ${value}.`);
  }
  return new Expr<T>(`node(${value}, ${quote(property)})`);
}

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

/** What `expr` interpolates: expressions, `true` and `false`, and the literals builtins take. */
export type ExprPart = AnyArg | boolean;

/**
 * A raw expression, such as `expr("text_width(node(3))")`, for forms the helpers don't cover.
 * `source` has no leading `=`. It takes the type of the param it is in, or the one you give, as
 * in `expr<number>(..)`. Given neither, as in `const x = expr("frame")`, it fits any param, and
 * only the engine checks its type.
 *
 * As a template tag, each interpolated value is lowered like a builtin argument, so built
 * expressions need no `=` stripped: ``expr<Vec2>`vec2(0, ${lift})` ``. Numbers, strings and
 * tuples are formatted and quoted, and a raw expression is bracketed where it meets an operator.
 * The text between interpolations is taken as written, so a backslash reaches the engine as a
 * backslash: ``expr`uppercase('it\'s')` `` keeps its `\'`. Only `` \` `` and `\${` lose theirs.
 */
export function expr<T extends Value = never>(source: string): Expr<T>;
export function expr<T extends Value = never>(
  strings: TemplateStringsArray,
  ...parts: readonly ExprPart[]
): Expr<T>;
export function expr<T extends Value = never>(
  source: string | TemplateStringsArray,
  ...parts: readonly ExprPart[]
): Expr<T> {
  const chunks =
    typeof source === "string"
      ? [source]
      : source.raw.map((chunk) => chunk.replace(/\\(`|\$\{)/g, "$1"));
  if (chunks[0]?.startsWith("=")) {
    throw new TypeError("`expr` takes the expression without its leading `=`.");
  }
  const expression = new Expr<T>(
    chunks.reduce((text, chunk, index) => `${text}${interpolated(parts[index - 1])}${chunk}`),
  );
  rawExpressions.add(expression);
  return expression;
}

function interpolated(part: unknown): string {
  if (typeof part === "boolean") {
    return String(part);
  }
  if (part instanceof Expr || typeof part === "number" || typeof part === "string") {
    return operand(part);
  }
  if (
    Array.isArray(part) &&
    part.every((member) => member instanceof Expr || typeof member === "number")
  ) {
    return lower(part as unknown as AnyArg);
  }
  throw new TypeError(`\`expr\` cannot interpolate ${part === null ? "null" : typeof part}.`);
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

const identifier = "[A-Za-z_][A-Za-z0-9_]*";

/** A name and a comma-separated list of parameter names, like the composition schema's pattern. */
const signaturePattern = new RegExp(
  `^\\s*(${identifier})\\s*\\(\\s*(${identifier}(?:\\s*,\\s*${identifier})*)?\\s*\\)\\s*$`,
);

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
  const match = signaturePattern.exec(signature);
  if (match === null) {
    throw new TypeError(
      `Invalid function signature \`${signature}\`; expected \`name(a, b)\` with identifiers for the name and parameters.`,
    );
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

export interface Move<T extends number | Vec2 | Vec3Value> {
  readonly by: Arg<T>;
  readonly start: Num;
  readonly duration: Num;
  readonly ease?: EasingArg;
}

/** `base` plus `by * tween(0, 1, ..)` per move, so moves can overlap. */
export function moves(base: Num, steps: readonly Move<number>[]): Expr<number>;
export function moves(base: Arg<Vec2>, steps: readonly Move<Vec2>[]): Expr<Vec2>;
export function moves(base: Arg<Vec3Value>, steps: readonly Move<Vec3Value>[]): Expr<Vec3Value>;
export function moves(
  base: Num | Arg<Vec2> | Arg<Vec3Value>,
  steps: readonly Move<number>[] | readonly Move<Vec2>[] | readonly Move<Vec3Value>[],
): Expr {
  const initial = base instanceof Expr ? base : new Expr(lower(base));
  const untypedTween = tween as (...args: readonly AnyArg[]) => Expr;
  return steps.reduce<Expr>((sum, { by, start, duration, ease }) => {
    const eased = untypedTween(0, 1, start, duration, ...(ease === undefined ? [] : [ease]));
    return binary(sum, "+", binary(by, "*", eased));
  }, initial);
}
