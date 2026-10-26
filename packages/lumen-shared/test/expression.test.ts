import { expressionBuiltins, expressionGlobals, type Vec2 } from "@lumiscia/lumen-types";
import { format } from "oxfmt";
import { describe, expect, test } from "vitest";

import * as shared from "../src/index.js";
import {
  Composition,
  abs,
  ceil,
  clamp,
  cos,
  cubicBezier,
  ease,
  floor,
  fn,
  fps,
  fract,
  frame,
  height,
  hex,
  inBack,
  inCubic,
  inExpo,
  inOutBack,
  inOutCubic,
  inOutExpo,
  inOutQuad,
  inQuad,
  lerp,
  linear,
  lowercase,
  max,
  min,
  mod,
  moves,
  ms,
  outBack,
  outCubic,
  outExpo,
  outQuad,
  pow,
  progress,
  rgb,
  rgba,
  round,
  sec,
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
  linear: { expression: linear(0, 100, 0.5), expected: 50 },
  step: { expression: step(1, 2, 1), expected: 2 },
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
  cubic_bezier: { expression: cubicBezier(0, 0, 1, 1, 0.5), expected: 0.5 },
  spring: { expression: spring(0, 170, 26), expected: 0 },
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
  const output = composition.addNode({ type: "media_output" });
  composition
    .connect(title, fade, { toPort: "source" })
    .connect(fade, output, { toPort: "source" });

  return {
    composition: composition.toJSON(),
    frame: 45,
    cases: Object.fromEntries(
      Object.entries(cases).map(([name, { expected, expression }]) => [
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
  });

  test("parenthesise every operation", () => {
    expect(String(frame.sub(12).div(fps).mul(2).oneMinus())).toBe(
      "=(1 - (((frame - 12) / fps) * 2))",
    );
  });

  test("format numbers deterministically", () => {
    expect(String(vec2(-0, 1e-7))).toBe("=vec2(0, 0.0000001)");
    expect(String(vec2(-1.5e21, 0.1 + 0.2))).toBe(
      "=vec2(-1500000000000000000000, 0.30000000000000004)",
    );
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

  test("drop trailing undefined arguments", () => {
    expect(String(tween(0, 1, 0, 12, undefined))).toBe("=tween(0, 1, 0, 12)");
    expect(String(spring(0, undefined, undefined))).toBe("=spring(0)");
    // @ts-expect-error interior holes are rejected at compile time too
    expect(() => spring(0, undefined, 20)).toThrow(
      "`spring` cannot skip an optional argument before a later one.",
    );
  });

  test("moves accepts expressions of unknown kind", () => {
    const offset = fn("offset(x)", (x: Num) => moves(x, [{ by: 10, start: 0, duration: 10 }]));
    expect(offset.body).toBe("(x + tween(0, 10, 0, 10))");
    expect(String(offset(frame).add(1))).toBe("=(offset(frame) + 1)");
    expect(fn("same(x)", (x: Num) => moves(x, [])).body).toBe("x");
    expect(fn("at(x)", (x: Num) => moves(x, [{ by: [1, 2], start: 0, duration: 10 }])).body).toBe(
      "(x + tween(vec2(0, 0), vec2(1, 2), 0, 10))",
    );
    expect(() =>
      fn("blind(x, y)", (x: Num, y: Num) => moves(x, [{ by: y, start: 0, duration: 10 }])),
    ).toThrow("`moves` cannot tell whether its values are numbers or vec2s; type the base.");
    expect(() => moves(hex("#fff000") as unknown as Num, [])).toThrow(
      "`moves` needs a number or vec2 base.",
    );
  });

  test("reject reserved and duplicate names in function signatures", () => {
    const body = (x: Num) => x as Expr<number>;
    expect(() => fn("frame(x)", body)).toThrow("`frame` is a reserved name.");
    expect(() => fn("tween(x)", body)).toThrow("`tween` is a reserved name.");
    expect(() => fn("f(frame)", body)).toThrow("`frame` is a reserved name.");
    expect(() => fn("f(if)", body)).toThrow("`if` is a reserved name.");
    expect(() => fn("f(f)", (f: Num) => f as Expr<number>)).toThrow(
      "Parameter `f` has the same name as its function.",
    );
    const g = fn("g()", () => frame);
    const f = fn("f(g)", (start: Num) => tween(0, 1, start, 12));
    expect(() => new Composition().addFunction(g).addFunction(f)).toThrow(
      "Parameter `g` of `f` has the same name as a function.",
    );
    expect(() => new Composition().addFunction(f).addFunction(g)).toThrow(
      "Parameter `g` of `f` has the same name as a function.",
    );
    expect(() => fn("f(x, x)", (x: Num, _y: Num) => x as Expr<number>)).toThrow(
      "Parameter `x` is declared twice.",
    );
  });

  test("cover every builtin and global", () => {
    const exports: Record<string, unknown> = shared;
    for (const { name } of expressionBuiltins) {
      const helper = name.replace(/_([a-z])/g, (_, char: string) => char.toUpperCase());
      expect(typeof exports[helper], helper).toBe("function");
      expect(cases[name]?.expression.toString(), name).toMatch(new RegExp(`^=${name}\\(`));
    }
    for (const { name } of expressionGlobals) {
      expect(String(exports[name]), name).toBe(`=${name}`);
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
