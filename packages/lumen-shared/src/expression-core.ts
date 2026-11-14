import {
  expressionBuiltins,
  type Color,
  type EasingName,
  type ExpressionBuiltinName,
  type ExpressionString,
  type Vec2,
} from "@lumiscia/lumen-types";

import { setLiteral } from "./literal.js";
import { quote } from "./quote.js";

// The part of the expression API that `generated/expressions.ts` builds on. It must not import that
// file, which is evaluated after this one.

declare const valueType: unique symbol;
declare const easingType: unique symbol;
declare const keyType: unique symbol;
declare const vec3Type: unique symbol;

/** A type an expression can evaluate to. Params take all but `boolean`, `Easing` and `Key`. */
export type Value = boolean | number | Vec2 | Vec3Value | Color | string | Easing | Key;

/**
 * What `vec3(..)` evaluates to. It is its own type rather than the `Vec3` tuple, which is also an
 * RGB `Color`, so vec3 and colour expressions don't mix.
 */
export interface Vec3Value {
  readonly [vec3Type]: true;
}

/** What `cubicBezier` evaluates to. `ease`, `tween`, `key` and `keyframes` take it; params don't. */
export interface Easing {
  readonly [easingType]: true;
}

/** An easing name such as `ease.outExpo`, or an easing such as `cubicBezier(..)`. */
export type EasingArg = EasingName | Expr<Easing> | Expr<string>;

/**
 * Strings that are path data interpolate when their commands match; any other string keys only
 * with the `hold` easing.
 */
export type KeyValue = number | Vec2 | Vec3Value | Color | string;

/** What `key` evaluates to: a keyframe holding a `T`. Only `keyframes` takes it. */
export interface Key<T extends KeyValue = KeyValue> {
  readonly [keyType]: T;
}

export type Num = number | Expr<number>;

/** What `when`, `and` and `or` test. The engine counts a number as true unless it is 0. */
export type Condition = Expr<boolean> | Expr<number>;

/**
 * An expression, or a literal of the same type. Tuples lower to `vec2(..)`, `rgb(..)` or
 * `rgba(..)`, so a vec3 has no tuple form; write `vec3(x, y, z)`.
 */
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
 * vec2 and vec3 values add and subtract with their own kind and scale by numbers or their own
 * kind, and colours have no arithmetic. Comparisons and logic evaluate to `boolean`, which `when`
 * branches on.
 */
export class Expr<T extends Value = Value> {
  declare readonly [valueType]: T;
  readonly #source: string;

  constructor(source: string) {
    this.#source = source;
  }

  add(this: Expr<number>, other: Num): Expr<number>;
  add(this: Expr<Vec2>, other: Arg<Vec2>): Expr<Vec2>;
  add(this: Expr<Vec3Value>, other: Arg<Vec3Value>): Expr<Vec3Value>;
  add(other: AnyArg): Expr {
    return binary(this, "+", other);
  }

  sub(this: Expr<number>, other: Num): Expr<number>;
  sub(this: Expr<Vec2>, other: Arg<Vec2>): Expr<Vec2>;
  sub(this: Expr<Vec3Value>, other: Arg<Vec3Value>): Expr<Vec3Value>;
  sub(other: AnyArg): Expr {
    return binary(this, "-", other);
  }

  mul(this: Expr<number>, other: Num): Expr<number>;
  mul(this: Expr<number>, other: Arg<Vec2>): Expr<Vec2>;
  mul(this: Expr<Vec2>, other: Num | Arg<Vec2>): Expr<Vec2>;
  mul(this: Expr<number>, other: Arg<Vec3Value>): Expr<Vec3Value>;
  mul(this: Expr<Vec3Value>, other: Num | Arg<Vec3Value>): Expr<Vec3Value>;
  mul(other: AnyArg): Expr {
    return binary(this, "*", other);
  }

  div(this: Expr<number>, other: Num): Expr<number>;
  div(this: Expr<number>, other: Arg<Vec2>): Expr<Vec2>;
  div(this: Expr<Vec2>, other: Num | Arg<Vec2>): Expr<Vec2>;
  div(this: Expr<number>, other: Arg<Vec3Value>): Expr<Vec3Value>;
  div(this: Expr<Vec3Value>, other: Num | Arg<Vec3Value>): Expr<Vec3Value>;
  div(other: AnyArg): Expr {
    return binary(this, "/", other);
  }

  oneMinus(this: Expr<number>): Expr<number> {
    return binary(1, "-", this);
  }

  clamp(this: Expr<number>, min: Num, max: Num): Expr<number> {
    return call("clamp", [this, min, max]);
  }

  gt(this: Expr<number>, other: Num): Expr<boolean> {
    return binary(this, ">", other);
  }

  gte(this: Expr<number>, other: Num): Expr<boolean> {
    return binary(this, ">=", other);
  }

  lt(this: Expr<number>, other: Num): Expr<boolean> {
    return binary(this, "<", other);
  }

  lte(this: Expr<number>, other: Num): Expr<boolean> {
    return binary(this, "<=", other);
  }

  /** Values of different types are never equal, so both sides must share a type. */
  eq(this: Expr<number>, other: Num): Expr<boolean>;
  eq(this: Expr<boolean>, other: Expr<boolean>): Expr<boolean>;
  eq(this: Expr<Vec2>, other: Arg<Vec2>): Expr<boolean>;
  eq(this: Expr<Vec3Value>, other: Arg<Vec3Value>): Expr<boolean>;
  eq(this: Expr<Color>, other: Arg<Color>): Expr<boolean>;
  eq(this: Expr<string>, other: Arg<string>): Expr<boolean>;
  eq(other: AnyArg): Expr<boolean> {
    return binary(this, "==", other);
  }

  /** Values of different types are never equal, so both sides must share a type. */
  neq(this: Expr<number>, other: Num): Expr<boolean>;
  neq(this: Expr<boolean>, other: Expr<boolean>): Expr<boolean>;
  neq(this: Expr<Vec2>, other: Arg<Vec2>): Expr<boolean>;
  neq(this: Expr<Vec3Value>, other: Arg<Vec3Value>): Expr<boolean>;
  neq(this: Expr<Color>, other: Arg<Color>): Expr<boolean>;
  neq(this: Expr<string>, other: Arg<string>): Expr<boolean>;
  neq(other: AnyArg): Expr<boolean> {
    return binary(this, "!=", other);
  }

  /** Skips evaluating `other` when this is false. */
  and(this: Expr<boolean>, other: Condition): Expr<boolean> {
    return binary(this, "&&", other);
  }

  /** Skips evaluating `other` when this is true. */
  or(this: Expr<boolean>, other: Condition): Expr<boolean> {
    return binary(this, "||", other);
  }

  not(this: Expr<boolean>): Expr<boolean> {
    return new Expr(`(!${operand(this)})`);
  }

  toString(): ExpressionString {
    return `=${this.#source}`;
  }

  toJSON(): ExpressionString {
    return this.toString();
  }
}

export function binary<T extends Value>(lhs: AnyArg, operator: string, rhs: AnyArg): Expr<T> {
  return new Expr<T>(`(${operand(lhs)} ${operator} ${operand(rhs)})`);
}

/** Expressions made by `expr`, whose source may have operators outside any brackets. */
export const rawExpressions = new WeakSet<Expr>();

/** `value` as one side of an operator, bracketed where its own operators could regroup. */
export function operand(value: AnyArg): string {
  const source = lower(value);
  return value instanceof Expr && rawExpressions.has(value) ? `(${source})` : source;
}

const tupleBuiltins: Readonly<Record<number, string>> = { 2: "vec2", 3: "rgb", 4: "rgba" };

export function lower(value: AnyArg): string {
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
export function formatNumber(value: number): string {
  if (!Number.isFinite(value)) {
    throw new RangeError(`Expressions cannot contain \`${value}\`.`);
  }
  return String(value);
}

const specs: ReadonlyMap<string, (typeof expressionBuiltins)[number]> = new Map(
  expressionBuiltins.map((spec) => [spec.name, spec]),
);

/** The expression `name(args)`, with trailing `undefined` arguments dropped. */
export function call<T extends Value = Value>(
  name: ExpressionBuiltinName,
  given: readonly (AnyArg | undefined)[],
): Expr<T> {
  const { maxArgs, minArgs } = specs.get(name) ?? { maxArgs: 0, minArgs: 0 };
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
  const expr = new Expr<T>(`${name}(${(args as readonly AnyArg[]).map(lower).join(", ")})`);
  const numbers = args.filter((arg): arg is number => typeof arg === "number");
  if ((name === "vec2" || name === "vec3") && numbers.length === args.length) {
    setLiteral(expr, numbers);
  }
  return expr;
}
