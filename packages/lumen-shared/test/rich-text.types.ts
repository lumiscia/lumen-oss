import type { ShaderUniforms, TextSpan } from "@lumiscia/lumen-types";

import {
  Composition,
  frame,
  hex,
  progress,
  uppercase,
  vec2,
  type ExpressionInput,
} from "../src/index.js";

const composition = new Composition();

const spans: readonly ExpressionInput<TextSpan>[] = [
  { text: uppercase("x"), font_size: frame.mul(2), color: hex("#ffffff") },
  {
    text: "plain",
    underline: { extent: vec2(0, progress(0, 30)), thickness: frame },
    highlight: null,
  },
];

composition.addNode({
  type: "text",
  params: {
    content: spans,
    underline: { extent: vec2(0, progress(0, 30)), style: "wavy" },
    strikethrough: { offset: frame.sub(4), paint: hex("#ff0000") },
    highlight: { paint: hex("#ffff00"), padding: vec2(4, 2), radius: frame },
  },
});

composition.addNode({
  type: "text",
  params: {
    content: [{ text: "a", highlight: { extent: vec2(0, 1) }, strikethrough: null }],
  },
});

composition.addNode({
  type: "text",
  params: {
    // @ts-expect-error a span's font size takes a number, not a colour
    content: [{ text: "a", font_size: hex("#ffffff") }],
  },
});

composition.addNode({
  type: "text",
  params: {
    // @ts-expect-error a span's text takes a string, not a number
    content: [{ text: frame }],
  },
});

composition.addNode({
  type: "text",
  params: {
    // @ts-expect-error an underline's extent takes a vec2, not a number
    underline: { extent: frame },
  },
});

composition.addNode({
  type: "text",
  params: {
    // @ts-expect-error a highlight's paint takes a colour, not a vec2
    highlight: { paint: vec2(0, 1) },
  },
});

composition.addNode({
  type: "text",
  params: {
    // @ts-expect-error a span's underline takes a line object, not an expression
    content: [{ text: "a", underline: frame }],
  },
});

// Each kind of uniform value takes its own expression.
const uniforms: ExpressionInput<ShaderUniforms> = { strength: frame, center: vec2(0, 0) };
void uniforms;

// @ts-expect-error a string expression is not a uniform value
const badUniforms: ExpressionInput<ShaderUniforms> = { label: uppercase("x") };
void badUniforms;
