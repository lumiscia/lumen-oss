/** The literal each `vec2(..)` or `vec3(..)` of plain numbers stands for. */
const literals = new WeakMap<object, readonly number[]>();

export function setLiteral(expr: object, value: readonly number[]): void {
  literals.set(expr, value);
}

/** The literal `expr` stands for, which a param takes in place of the expression. */
export function literalOf(expr: object): number[] | undefined {
  const value = literals.get(expr);
  return value === undefined ? undefined : [...value];
}
