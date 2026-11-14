import { cos, sin, vec3, type Expr, type Num, type Vec3Value } from "@lumiscia/lumen-shared";

/** A point on a level circle round the origin, `angle` radians round from +Z. */
export function orbit(angle: Expr<number>, radius: number, height: Num): Expr<Vec3Value> {
  return vec3(sin(angle).mul(radius), height, cos(angle).mul(radius));
}
