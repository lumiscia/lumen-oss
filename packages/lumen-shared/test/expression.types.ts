import type { Color, Vec2 } from "@lumiscia/lumen-types";

import {
  Composition,
  ease,
  fn,
  frame,
  hex,
  lerp,
  moves,
  rgb,
  spring,
  textWidth,
  lowercase,
  tween,
  vec2,
  type Expr,
  type Num,
} from "../src/index.js";

const color: Expr<Color> = hex("#ff0000");
const point: Expr<Vec2> = vec2(frame, 0);
const number: Expr<number> = frame.add(1);

// Valid calls.
const scalar: Expr<number> = tween(0, 1, 0, 12, ease.outExpo);
const position: Expr<Vec2> = tween([0, 0], point, 0, 12);
const tint: Expr<Color> = lerp(color, [255, 0, 0], 0.5);
const slid: Expr<Vec2> = moves(point, [{ by: [10, 0], start: 0, duration: 10 }]);
const rise = fn("rise(start)", (start: Num) => tween(0, 1, start, 12));
const composition = new Composition().addFunction(rise);
composition.addNode({
  type: "path",
  params: { fill_paint: color, position: point, stroke_width: number },
});
void [scalar, position, tint, slid, composition, spring(1), rgb(1, 2, 3)];

// @ts-expect-error numbers and colours don't combine
number.add(color);

// @ts-expect-error colours have no arithmetic
color.add(color);

// @ts-expect-error `from` and `to` must share a kind
tween(0, color, 0, 12);

// @ts-expect-error a number is not a colour
rgb(1, 2, 3).add(number);

new Composition().addNode({
  type: "path",
  // @ts-expect-error a non-colour expression can't paint
  params: { fill_paint: number },
});

new Composition().addNode({
  type: "path",
  // @ts-expect-error a number expression can't be a vec2
  params: { position: number },
});

new Composition().addNode({
  type: "alpha_premultiply",
  // @ts-expect-error constant string params don't take expressions
  params: { mode: lowercase("PREMULTIPLY") },
});

new Composition().addNode({
  type: "text",
  params: { content: lowercase("HELLO") },
});

// @ts-expect-error an optional argument can't be skipped before a later one
spring(0, undefined, 20);

// @ts-expect-error holes after the first optional argument are rejected too
textWidth("a", undefined, 100);

// @ts-expect-error a skipped argument must stay skipped
textWidth("a", undefined, 100, "Inter");

// Trailing and optional-variable `undefined` is fine.
const stiffness: number | undefined = undefined as number | undefined;
void [
  textWidth("a", undefined, undefined, undefined),
  textWidth("a", 12, undefined),
  spring(0, undefined, undefined),
  spring(0, stiffness),
  tween(0, 1, 0, 12, undefined),
];
