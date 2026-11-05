import { describe, expect, test } from "vitest";

import { Composition } from "../src/composition.js";
import { frame, rgb, time } from "../src/expression.js";
import { wgslShader } from "../src/shader.js";

const body = `diagnostic(off, derivative_uniformity);

@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    textureStore(output, vec2<i32>(id.xy), uniforms.tint * uniforms.strength);
}`;

describe("wgslShader", () => {
  test("appends the uniform struct after directives and code and emits node JSON", () => {
    const ripple = wgslShader({
      uniforms: { strength: "f32", center: "vec2<f32>", tint: "vec4<f32>", steps: "u32" },
      shader: body,
    });
    const composition = new Composition();
    composition.addNode({
      ...ripple.node({ strength: 0.5, center: [960, "=frame"], tint: "#ff0000", steps: 4 }),
      id: 3,
    });

    expect(ripple.source).toBe(`${body}

struct Uniforms {
    strength: f32,
    center: vec2<f32>,
    tint: vec4<f32>,
    steps: u32,
}
@group(0) @binding(1) var<uniform> uniforms: Uniforms;
`);
    expect(composition.toJSON().nodes).toEqual([
      {
        id: 3,
        type: "wgsl_shader",
        params: {
          shader: ripple.source,
          uniforms: { strength: 0.5, center: [960, "=frame"], tint: "#ff0000", steps: 4 },
        },
      },
    ]);
  });

  test("lowers expression values and components", () => {
    const ripple = wgslShader({
      uniforms: { strength: "f32", center: "vec2<f32>", tint: "vec3<f32>" },
      shader: body,
    });

    expect(
      ripple.node({ strength: time.mul(2), center: [960, frame], tint: rgb(255, 0, 51) }).params
        ?.uniforms,
    ).toEqual({ strength: "=(time * 2)", center: [960, "=frame"], tint: "=rgb(255, 0, 51)" });
  });

  test("leaves shaders without uniforms untouched", () => {
    const shader = `@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    textureStore(output, vec2<i32>(id.xy), vec4<f32>(1.0, 0.0, 0.0, 1.0));
}`;
    const fill = wgslShader({ shader });

    expect(fill.source).toBe(shader);
    expect(fill.node()).toEqual({ type: "wgsl_shader", params: { shader, uniforms: {} } });
  });

  test("rejects alpha on vec3 colours", () => {
    const tint = wgslShader({ uniforms: { tint: "vec3<f32>" }, shader: body });
    expect(() => tint.node({ tint: "#ff000080" })).toThrow(
      "Uniform `tint` is a vec3, so its colour cannot have alpha.",
    );
  });
});
