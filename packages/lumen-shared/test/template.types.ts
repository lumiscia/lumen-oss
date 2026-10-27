import { frame, hex, progress, rgb, uppercase, vec2 } from "../src/expression.js";
import { defineComposition, prop } from "../src/template.js";

const titleCard = defineComposition({
  name: "title_card",
  size: { width: 960, height: 360 },
  props: {
    title: prop.string("Untitled"),
    accent: prop.color("#ee5b24"),
    index: prop.number(1),
    offset: prop.vec2(),
  },
  build(c, p) {
    const label = c.addNode({
      type: "text",
      params: { content: p.title, color: p.accent, position: p.offset.add([0, 40]) },
    });
    // @ts-expect-error a string prop is not a number
    c.addNode({ type: "text", params: { font_size: p.title } });
    return label;
  },
});

titleCard.clip({ start: 30, props: { title: "Shapes", accent: [255, 0, 0], index: 2 } });
titleCard.clip({ start: frame.mul(2), props: { title: uppercase("x"), accent: hex("#22d3ee") } });
titleCard.clip({ props: { accent: rgb(1, 2, 3), index: progress(0, 10), offset: vec2(1, 2) } });
titleCard.clip({ end: "hold", speed: 1.5, offset: 10, duration: 60 });

// @ts-expect-error unknown prop
titleCard.clip({ props: { titel: "Shapes" } });
// @ts-expect-error a number prop takes a number
titleCard.clip({ props: { index: "two" } });
// @ts-expect-error a number prop takes a number expression
titleCard.clip({ props: { index: uppercase("two") } });
// @ts-expect-error a colour prop takes a colour
titleCard.clip({ props: { accent: [1, 2] } });
// @ts-expect-error unknown end mode
titleCard.clip({ end: "freeze" });
const plain = defineComposition({ name: "plain", build: (c) => c.addNode({ type: "background" }) });
// @ts-expect-error templates without props take none
plain.clip({ props: { title: "x" } });
// @ts-expect-error defaults match the declared type
prop.number("1");
