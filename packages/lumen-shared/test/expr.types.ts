import { Composition } from "../src/composition.js";
import { expr } from "../src/expression.js";

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
