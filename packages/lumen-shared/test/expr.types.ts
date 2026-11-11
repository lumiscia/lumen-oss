import type { Vec2 } from "@lumiscia/lumen-types";

import { Composition } from "../src/composition.js";
import { expr, frame } from "../src/expression.js";

const composition = new Composition();

// A raw expression takes the type of the param it is in.
composition.addNode({
  type: "text",
  params: {
    content: expr("if(frame > 30, 'a', 'b')"),
    opacity: expr("if(frame > 30, 1, 0)"),
    position: expr("vec2(frame, 0)"),
  },
});
composition.addNode({
  type: "opacity",
  params: { opacity: expr<number>("node(3, 'opacity')") },
});

// @ts-expect-error a number expression is not a position
composition.addNode({ type: "text", params: { position: expr<number>("frame") } });

// Interpolations take expressions, booleans, and the literals builtins take.
composition.addNode({
  type: "text",
  params: { position: expr<Vec2>`vec2(0, ${frame}) + ${[1, 2]}` },
});
composition.addNode({
  type: "opacity",
  params: { opacity: expr<number>`if(${true}, ${1}, ${"1"})` },
});

// @ts-expect-error a number expression is not a position
composition.addNode({ type: "text", params: { position: expr<number>`${frame}` } });

// @ts-expect-error objects can't be interpolated
void expr`${{ x: 1 }}`;

// @ts-expect-error undefined can't be interpolated
void expr`${undefined}`;

// @ts-expect-error tuples hold numbers and number expressions
void expr`${["a", "b"]}`;
