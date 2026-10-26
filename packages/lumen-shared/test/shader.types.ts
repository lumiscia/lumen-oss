import { frame, hex, rgba, time, vec2 } from "../src/expression.js";
import { wgslShader } from "../src/shader.js";

const ripple = wgslShader({
  uniforms: {
    strength: "f32",
    center: "vec2<f32>",
    tint: "vec4<f32>",
    steps: "u32",
    grid: "vec3<i32>",
  },
  shader: "",
});

ripple.node({ strength: "=time", center: ["=frame", 540], tint: "#ff0000", steps: 4 });
ripple.node({ center: "=vec2(1, 2)", tint: [1, 0, 0, 1], grid: [1, 2, 3] });
ripple.node({ strength: time.mul(2), center: vec2(1, 2), tint: rgba(255, 0, 0, 128) });
ripple.node({ center: [frame, 540], tint: hex("#ff000080") });

// @ts-expect-error scalars take a number or an expression
ripple.node({ strength: [1, 2] });
// @ts-expect-error vec2 needs exactly two components
ripple.node({ center: [1, 2, 3] });
// @ts-expect-error only vec3<f32> and vec4<f32> take hex colours
ripple.node({ grid: "#ff0000" });
// @ts-expect-error unknown uniform
ripple.node({ missing: 1 });
// @ts-expect-error unsupported WGSL type
wgslShader({ uniforms: { m: "mat4x4<f32>" }, shader: "" });
// @ts-expect-error vec3 and vec4 take components, not a whole expression
ripple.node({ tint: "=vec2(1, 2)" });
// @ts-expect-error vec4 takes a colour expression, not a vec2 one
ripple.node({ tint: vec2(1, 2) });
// @ts-expect-error only vec3<f32> and vec4<f32> take colour expressions
ripple.node({ grid: rgba(1, 2, 3, 4) });
// @ts-expect-error shaders without uniforms take none
wgslShader({ shader: "" }).node({ missing: 1 });
// @ts-expect-error an empty declaration takes none either
wgslShader({ uniforms: {}, shader: "" }).node({ missing: 1 });
