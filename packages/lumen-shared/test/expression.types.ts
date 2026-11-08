import type { Color, Vec2 } from "@lumiscia/lumen-types";

import {
  Composition,
  cubicBezier,
  ease,
  fn,
  frame,
  hex,
  key,
  keyframes,
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
const bezier = cubicBezier([0.4, 0], [0.2, 1]);
const keyed: Expr<number> = keyframes(frame, key(0, 0), key(30, 1, bezier));
const eased: Expr<Vec2> = keyframes(frame, ease.outExpo, key(0, [0, 0]), key(30, point, "hold"));
const curve: Expr<number> = ease(bezier, 0.5);
const bent: Expr<number> = tween(0, 1, 0, 12, bezier);
void [scalar, position, tint, slid, composition, spring(1), rgb(1, 2, 3)];
void [keyed, eased, curve, bent];

// @ts-expect-error every key holds the same kind of value
keyframes(frame, key(0, 0), key(30, point));

// @ts-expect-error `keyframes` needs at least one key
keyframes(frame, ease.outExpo);

// @ts-expect-error `cubicBezier` takes two vec2 handles
cubicBezier(0.4, 0, 0.2, 1, 0.5);

// @ts-expect-error an unknown easing name
tween(0, 1, 0, 12, "bounce");

new Composition().addNode({
  type: "opacity",
  // @ts-expect-error an easing is not a param value
  params: { opacity: bezier },
});

new Composition().addNode({
  type: "opacity",
  // @ts-expect-error a key is not a param value
  params: { opacity: key(0, 1) },
});

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
  type: "shape",
  // @ts-expect-error constant string params don't take expressions
  params: { geometry_kind: lowercase("ELLIPSE") },
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
