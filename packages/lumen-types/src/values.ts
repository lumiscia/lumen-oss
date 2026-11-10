export type ExpressionString = `=${string}`;
// The intersection keeps `string` from absorbing the expression member, so consumers can tell
// an expression-capable string param from a constant one.
export type ExpressionValue<T> = T | (ExpressionString & {});
export type HexColor = `#${string}`;
export type Vec2 = readonly [number, number];
/**
 * A literal `[x, y, z]`. Expression helpers take `vec3(x, y, z)` instead, an `Expr<Vec3Value>`, since
 * a three-number tuple there is `rgb(..)`.
 */
export type Vec3 = readonly [number, number, number];
export type Color =
  | readonly [number, number, number]
  | readonly [number, number, number, number]
  | HexColor;
