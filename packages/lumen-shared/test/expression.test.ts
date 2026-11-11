import {
  expressionBuiltins,
  expressionGlobals,
  type Color,
  type Vec2,
} from "@lumiscia/lumen-types";
import { format } from "oxfmt";
import { describe, expect, test } from "vitest";

import * as shared from "../src/index.js";
import {
  Composition,
  abs,
  atan2,
  ceil,
  clamp,
  cos,
  cubicBezier,
  defineComposition,
  ease,
  expr,
  floor,
  fn,
  fps,
  fract,
  frame,
  glyph,
  height,
  hex,
  hypot,
  inBack,
  inBounce,
  inCirc,
  inCubic,
  inElastic,
  inExpo,
  inOutBack,
  inOutBounce,
  inOutCirc,
  inOutCubic,
  inOutElastic,
  inOutExpo,
  inOutQuad,
  inOutQuart,
  inOutQuint,
  inOutSine,
  inQuad,
  inQuart,
  inQuint,
  inSine,
  key,
  keyframes,
  lerp,
  literal,
  lowercase,
  max,
  min,
  mod,
  moves,
  ms,
  node,
  noise,
  outBack,
  outBounce,
  outCirc,
  outCubic,
  outElastic,
  outExpo,
  outQuad,
  outQuart,
  outQuint,
  outSine,
  pathAngle,
  pathPoint,
  pow,
  progress,
  prop,
  random,
  remap,
  rgb,
  rgba,
  round,
  sec,
  sign,
  sin,
  smoothstep,
  spring,
  step,
  textHeight,
  textWidth,
  time,
  tween,
  uppercase,
  vec2,
  when,
  width,
  type Arg,
  type Expr,
  type Num,
} from "../src/index.js";

const rise = fn("rise(start)", (start: Num) =>
  tween(vec2(0, 60), vec2(0, 0), start, 16, ease.outExpo),
);
const bob = fn("bob(start, offset)", (start: Num, offset: Arg<Vec2>) => rise(start).add(offset));

interface Case {
  readonly expression: Expr;
  /** Numbers, `[x, y]` for vec2 values, `#rrggbbaa` for colours, or strings. Omit to only evaluate. */
  readonly expected?: number | Vec2 | string;
}

/** Evaluated at frame 45 of a 30 fps, 1920x1080 composition. */
const cases: Readonly<Record<string, Case>> = {
  min: { expression: min(3, frame, 7), expected: 3 },
  max: { expression: max(1, 2), expected: 2 },
  abs: { expression: abs(-2.5), expected: 2.5 },
  floor: { expression: floor(2.7), expected: 2 },
  ceil: { expression: ceil(2.1), expected: 3 },
  round: { expression: round(2.5), expected: 3 },
  sin: { expression: sin(0), expected: 0 },
  cos: { expression: cos(0), expected: 1 },
  clamp: { expression: clamp(frame, 0, 30), expected: 30 },
  lerp: { expression: lerp(vec2(0, 10), [10, 30], 0.25), expected: [2.5, 15] },
  pow: { expression: pow(2, 10), expected: 1024 },
  mod: { expression: mod(-7, 3), expected: -1 },
  fract: { expression: fract(2.25), expected: 0.25 },
  smoothstep: { expression: smoothstep(0, 1, 0.5), expected: 0.5 },
  step: { expression: step(1, 2, 1), expected: 2 },
  sign: { expression: sign(-3), expected: -1 },
  atan2: { expression: atan2(1, 0), expected: Math.PI / 2 },
  hypot: { expression: hypot(3, 4), expected: 5 },
  remap: { expression: remap(frame, 0, 90, 100, 200), expected: 150 },
  text_height: { expression: textHeight("Hello") },
  text_width: { expression: textWidth("Hello", 32, 300, "Roboto") },
  uppercase: { expression: uppercase("it's a\\b\n"), expected: "IT'S A\\B\n" },
  lowercase: { expression: lowercase("ABC"), expected: "abc" },
  vec2: { expression: vec2(1, -2), expected: [1, -2] },
  rgb: { expression: rgb(255, 128, 0), expected: "#ff8000ff" },
  rgba: { expression: rgba(0, 0, 255, 51), expected: "#0000ff33" },
  progress: { expression: progress(30, 60), expected: 0.5 },
  tween: { expression: tween(hex("#000000"), [255, 255, 255], 30, 30), expected: "#808080ff" },
  in_quad: { expression: inQuad(0.5), expected: 0.25 },
  out_quad: { expression: outQuad(0.5), expected: 0.75 },
  in_out_quad: { expression: inOutQuad(0.25), expected: 0.125 },
  in_cubic: { expression: inCubic(0.5), expected: 0.125 },
  out_cubic: { expression: outCubic(0.5), expected: 0.875 },
  in_out_cubic: { expression: inOutCubic(0.5), expected: 0.5 },
  in_expo: { expression: inExpo(1), expected: 1 },
  out_expo: { expression: outExpo(0), expected: 0 },
  in_out_expo: { expression: inOutExpo(0.5), expected: 0.5 },
  in_back: { expression: inBack(0), expected: 0 },
  out_back: { expression: outBack(1), expected: 1 },
  in_out_back: { expression: inOutBack(0.5), expected: 0.5 },
  in_sine: { expression: inSine(0.5), expected: 1 - Math.SQRT1_2 },
  out_sine: { expression: outSine(0.5), expected: Math.SQRT1_2 },
  in_out_sine: { expression: inOutSine(0.5), expected: 0.5 },
  in_quart: { expression: inQuart(0.5), expected: 0.0625 },
  out_quart: { expression: outQuart(0.5), expected: 0.9375 },
  in_out_quart: { expression: inOutQuart(0.25), expected: 0.03125 },
  in_quint: { expression: inQuint(0.5), expected: 0.03125 },
  out_quint: { expression: outQuint(0.5), expected: 0.96875 },
  in_out_quint: { expression: inOutQuint(0.25), expected: 0.015625 },
  in_circ: { expression: inCirc(0.6), expected: 0.2 },
  out_circ: { expression: outCirc(0.8), expected: Math.sqrt(0.96) },
  in_out_circ: { expression: inOutCirc(0.5), expected: 0.5 },
  in_elastic: { expression: inElastic(0.5), expected: -0.015625 },
  out_elastic: { expression: outElastic(0.5), expected: 1.015625 },
  in_out_elastic: { expression: inOutElastic(0.5), expected: 0.5 },
  in_bounce: { expression: inBounce(0.5), expected: 0.234375 },
  out_bounce: { expression: outBounce(0.5), expected: 0.765625 },
  in_out_bounce: { expression: inOutBounce(0.25), expected: 0.1171875 },
  ease: { expression: ease(ease.outQuad, 0.5), expected: 0.75 },
  cubic_bezier: { expression: ease(cubicBezier([0, 0], [1, 1]), 0.5), expected: 0.5 },
  key: { expression: keyframes(frame, key(30, 0), key(60, 10)), expected: 5 },
  keyframes: {
    expression: keyframes(
      frame,
      ease.inQuad,
      key(sec(1), [0, 0]),
      key(sec(2), vec2(100, 200), ease.hold),
      key(sec(3), [0, 0]),
    ),
    expected: [25, 50],
  },
  colour_keys: {
    expression: keyframes(frame, key(30, hex("#ff000000")), key(60, hex("#0000ff"))),
    expected: "#0000ff80",
  },
  spring: { expression: spring(0, 170, 26), expected: 0 },
  noise: { expression: noise(1, 0.5), expected: -0.37396682188384345 },
  random: { expression: random(7, 0.5), expected: 0.7553920660716886 },
  path_point: { expression: pathPoint("M 0 0 L 100 0 L 100 300", 0.5), expected: [100, 100] },
  path_angle: { expression: pathAngle("M 0 0 L 100 0 L 100 300", 0.5), expected: 90 },
  path_lerp: {
    expression: lerp("M 0 0 L 100 0 Z", "M 0 40 L 100 40 Z", 0.25),
    expected: "M 0 10 L 100 10 Z",
  },
  path_keys: {
    expression: keyframes(frame, key(30, "M 0 0 H 10"), key(60, "M 0 20 H 30", ease.inQuad)),
    expected: "M 0 10 H 20",
  },
  prop: { expression: prop("title"), expected: "Lumen" },
  issue_position: { expression: vec2(160, 900).add(rise(570)), expected: [160, 960] },
  issue_opacity: { expression: tween(0, 1, sec(1.5), ms(300), ease.outExpo), expected: 0 },
  nested_function: { expression: bob(100, [5, 0]), expected: [5, 60] },
  vec2_arithmetic: {
    expression: vec2(1, 2).add([10, 20]).mul(2).sub(vec2(2, 4)),
    expected: [20, 40],
  },
  scalar_vec2: { expression: frame.mul(vec2(1, 2)).div(fps.div([3, 2])), expected: [4.5, 6] },
  one_minus_clamp: { expression: progress(30, 60).oneMinus().clamp(0, 0.25), expected: 0.25 },
  units: { expression: sec(1.5).add(ms(300)).sub(sec(-0.5)), expected: 69 },
  globals: { expression: frame.add(time).add(fps).add(width).add(height), expected: 3076.5 },
  moves_vec2: {
    expression: moves(vec2(960, 1200), [
      { by: vec2(0, -660), start: 0, duration: 18, ease: ease.outExpo },
      { by: [-1360, 0], start: 90, duration: 14, ease: ease.inBack },
    ]),
    expected: [960, 540],
  },
  moves_number: { expression: moves(100, [{ by: 50, start: 30, duration: 30 }]), expected: 125 },
  number_formatting: {
    expression: vec2(0.0000001, 1e21).add([-0, 0.1 + 0.2]),
    expected: [0.0000001, 1e21 + 0.30000000000000004],
  },
  when_number: {
    expression: when(frame.gte(30).and(frame.lt(60)), 1, 0.35),
    expected: 1,
  },
  when_colour: {
    expression: when(frame.gt(60), hex("#ff0000"), [0, 0, 255]),
    expected: "#0000ffff",
  },
  when_vec2: { expression: when(frame.lte(44), [1, 2], vec2(3, frame)), expected: [3, 45] },
  when_string: { expression: when(vec2(1, 2).eq([1, 2]), "yes", "no"), expected: "yes" },
  when_number_condition: { expression: when(mod(frame, 2), 7, 8), expected: 7 },
  when_logic: {
    expression: when(frame.lt(10).or(frame.eq(45)).and(frame.neq(3)).and(frame.gt(60).not()), 2, 3),
    expected: 2,
  },
  when_boolean_eq: { expression: when(frame.gt(1).eq(frame.gt(2)), 1, 0), expected: 1 },
  when_colour_neq: {
    expression: when(hex("#ff0000").neq([255, 0, 0]), 1, 0),
    expected: 0,
  },
  colour_tuple: { expression: lerp([255, 0, 0], hex("#0000ff80"), 0), expected: "#ff0000ff" },
};

function fixture() {
  const composition = new Composition({
    renderSettings: { height: 1080, width: 1920 },
    timeline: { duration_frames: 120, fps: 30 },
  })
    .addFunction(rise)
    .addFunction(bob);
  const title = composition.addNode({
    type: "text",
    params: {
      color: tween(hex("#ffffff00"), hex("#ffffff"), 0, 12),
      content: uppercase("Lumen"),
      font_size: frame.mul(0.5).add(32),
      position: vec2(160, 900).add(rise(570)),
    },
  });
  const fade = composition.addNode({
    type: "opacity",
    params: { opacity: tween(0, 1, sec(1.5), ms(300), ease.outExpo) },
  });
  const card = defineComposition({
    name: "card",
    props: { title: prop.string("Lumen"), offset: prop.vec2([0, 40]) },
    build: (c, p) =>
      c.addNode({ type: "text", params: { content: uppercase(p.title), position: p.offset } }),
  });
  composition.addNode(card.clip({ start: sec(1), props: { offset: vec2(frame, 0) } }));
  const output = composition.addNode({ type: "media_output" });
  composition
    .connect(title, fade, { toPort: "source" })
    .connect(fade, output, { toPort: "source" });

  return {
    composition: composition.toJSON(),
    frame: 45,
    cases: Object.fromEntries(
      Object.entries({
        ...cases,
        node: { expression: node(title, "font_size").sub(54), expected: 0.5 },
      }).map(([name, { expected, expression }]) => [
        name,
        { expression: expression.toString(), ...(expected === undefined ? {} : { expected }) },
      ]),
    ),
  };
}

describe("expression helpers", () => {
  test("emit the issue examples", () => {
    expect(String(vec2(160, 900).add(rise(570)))).toBe("=(vec2(160, 900) + rise(570))");
    expect(String(tween(0, 1, sec(1.5), ms(300), ease.outExpo))).toBe(
      "=tween(0, 1, 1.5s, 300ms, 'out_expo')",
    );
    expect(rise.signature).toBe("rise(start)");
    expect(rise.body).toBe("tween(vec2(0, 60), vec2(0, 0), start, 16, 'out_expo')");
    expect(String(tween(0, 1, glyph.mul(2).add(10), 12))).toBe(
      "=tween(0, 1, ((glyph * 2) + 10), 12)",
    );
  });

  test("parenthesise every operation", () => {
    expect(String(frame.sub(12).div(fps).mul(2).oneMinus())).toBe(
      "=(1 - (((frame - 12) / fps) * 2))",
    );
  });

  test("emit comparisons, logic and conditions", () => {
    expect(String(frame.gte(10).and(frame.lt(20)))).toBe("=((frame >= 10) && (frame < 20))");
    expect(String(frame.gt(1).or(frame.lte(0)).not())).toBe("=(!((frame > 1) || (frame <= 0)))");
    expect(String(frame.eq(3).neq(frame.eq(4)))).toBe("=((frame == 3) != (frame == 4))");
    expect(String(vec2(1, 2).eq([1, 2]))).toBe("=(vec2(1, 2) == vec2(1, 2))");
    expect(String(hex("#ff0000").neq([255, 0, 0]))).toBe("=(#ff0000 != rgb(255, 0, 0))");
    expect(String(uppercase("a").eq("A"))).toBe("=(uppercase('a') == 'A')");
    expect(String(frame.gt(1).and(mod(frame, 2)))).toBe("=((frame > 1) && mod(frame, 2))");
  });

  test("when lowers to if with the branch type", () => {
    expect(String(when(frame.gte(30).and(frame.lt(60)), 1, 0))).toBe(
      "=if(((frame >= 30) && (frame < 60)), 1, 0)",
    );
    expect(String(when(mod(frame, 2), "a", "b"))).toBe("=if(mod(frame, 2), 'a', 'b')");
    const tint: Expr<Color> = when(frame.gt(30), hex("#ff0000"), [0, 0, 255]);
    expect(String(tint)).toBe("=if((frame > 30), #ff0000, rgb(0, 0, 255))");
    const nested: Expr<Vec2> = when(frame.lt(10), [0, 0], when(frame.lt(20), [1, 1], vec2(2, 2)));
    expect(String(nested)).toBe(
      "=if((frame < 10), vec2(0, 0), if((frame < 20), vec2(1, 1), vec2(2, 2)))",
    );
    expect(String(when(frame.gt(1), 1, 0).add(1))).toBe("=(if((frame > 1), 1, 0) + 1)");
  });

  test("node reads a param of another node", () => {
    expect(String(node(3, "opacity"))).toBe("=node(3, 'opacity')");
    expect(String(node({ id: 7 }, "it's").add(1))).toBe("=(node(7, 'it\\'s') + 1)");
    expect(String(node<number>(2, "font_size").gt(20))).toBe("=(node(2, 'font_size') > 20)");
    expect(() => node(-1, "opacity")).toThrow("unsigned integer id");
    expect(() => node(1.5, "opacity")).toThrow("unsigned integer id");
  });

  test("format numbers deterministically", () => {
    expect(String(vec2(-0, 1e-7))).toBe("=vec2(0, 1e-7)");
    expect(String(vec2(-1.5e21, 0.1 + 0.2))).toBe("=vec2(-1.5e+21, 0.30000000000000004)");
    expect(() => vec2(Number.NaN, 0)).toThrow("Expressions cannot contain `NaN`.");
  });

  test("reject invalid calls and definitions", () => {
    expect(() => (min as (...args: Num[]) => Expr)(1)).toThrow(
      "`min` expects at least 2 arguments, got 1.",
    );
    expect(() => hex("#fff")).toThrow("Invalid colour `#fff`");
    expect(() => fn("rise(start, end)", (start: Num) => start as Expr<number>)).toThrow(
      "`rise` declares 2 parameters, but its implementation takes 1.",
    );
    expect(() =>
      new Composition().addFunction(rise).addFunction(fn("rise()", () => frame)),
    ).toThrow("Function `rise` already exists.");
  });

  test("emit keyframes with and without a default easing", () => {
    const bezier = cubicBezier([0.4, 0], [0.2, 1]);
    expect(String(bezier)).toBe("=cubic_bezier(vec2(0.4, 0), vec2(0.2, 1))");
    expect(String(keyframes(frame, ease.inOutCubic, key(0, 0), key(30, 1, bezier)))).toBe(
      "=keyframes(frame, 'in_out_cubic', key(0, 0), key(30, 1, cubic_bezier(vec2(0.4, 0), vec2(0.2, 1))))",
    );
    expect(String(keyframes(frame, key(0, [0, 0]), key(sec(1), [1, 1], "hold")))).toBe(
      "=keyframes(frame, key(0, vec2(0, 0)), key(1s, vec2(1, 1), 'hold'))",
    );
    expect(String(tween(0, 1, 0, 12, bezier))).toBe(
      "=tween(0, 1, 0, 12, cubic_bezier(vec2(0.4, 0), vec2(0.2, 1)))",
    );
  });

  test("drop trailing undefined arguments", () => {
    expect(String(tween(0, 1, 0, 12, undefined))).toBe("=tween(0, 1, 0, 12)");
    expect(String(spring(0, undefined, undefined))).toBe("=spring(0)");
    // @ts-expect-error interior holes are rejected at compile time too
    expect(() => spring(0, undefined, 20)).toThrow(
      "`spring` cannot skip an optional argument before a later one.",
    );
  });

  test("moves adds each move as its distance times a tween from 0 to 1", () => {
    const offset = fn("offset(x)", (x: Num) => moves(x, [{ by: 10, start: 0, duration: 10 }]));
    expect(offset.body).toBe("(x + (10 * tween(0, 1, 0, 10)))");
    expect(String(offset(frame).add(1))).toBe("=(offset(frame) + 1)");
    expect(fn("same(x)", (x: Num) => moves(x, [])).body).toBe("x");
    expect(
      String(moves(vec2(1, 2), [{ by: [3, 4], start: 5, duration: 10, ease: ease.outExpo }])),
    ).toBe("=(vec2(1, 2) + (vec2(3, 4) * tween(0, 1, 5, 10, 'out_expo')))");
  });

  test("literal escapes a leading = and expr wraps raw expression text", () => {
    expect(literal("=x")).toBe("==x");
    expect(literal("x=")).toBe("x=");
    expect(String(expr("if(frame > 30, 1, 0)"))).toBe("=if(frame > 30, 1, 0)");
    expect(() => expr("=frame")).toThrow("without its leading `=`");
  });

  test("cover every builtin and global", () => {
    const exports: Record<string, unknown> = shared;
    for (const { name } of expressionBuiltins) {
      const helper = name.replace(/_([a-z])/g, (_, char: string) => char.toUpperCase());
      expect(typeof exports[helper], helper).toBe("function");
      expect(cases[name]?.expression.toString(), name).toMatch(new RegExp(`\\b${name}\\(`));
    }
    for (const { name } of expressionGlobals) {
      const helper = name.replace(/_([a-z])/g, (_, char: string) => char.toUpperCase());
      expect(String(exports[helper]), helper).toBe(`=${name}`);
    }
  });

  // Regenerate with `just generate-expression-fixture`. `crates/engine/tests/ts_expressions.rs`
  // parses and evaluates every expression in it.
  test("round-trip fixture is fresh", async () => {
    const { code } = await format("ts_expressions.json", JSON.stringify(fixture()));
    await expect(code).toMatchFileSnapshot(
      "../../../crates/engine/tests/fixtures/ts_expressions.json",
    );
  });
});
