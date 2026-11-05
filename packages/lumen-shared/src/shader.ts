import type { ShaderUniforms } from "@lumiscia/lumen-types";

import { Expr } from "./expression.js";
import type { WgslShader, WgslShaderOptions, WgslUniforms } from "./types.js";

export function wgslShader<TUniforms extends WgslUniforms = Record<never, never>>(
  options: WgslShaderOptions<TUniforms>,
): WgslShader<TUniforms> {
  const uniforms: WgslUniforms = options.uniforms ?? {};
  // Appended, since WGSL directives must lead the module and declarations may come in any order.
  const declarations = uniformDeclarations(uniforms);
  const source = declarations === "" ? options.shader : `${options.shader}\n\n${declarations}`;
  return {
    source,
    node: (values = {}) => {
      for (const [name, value] of Object.entries(values)) {
        if (uniforms[name]?.startsWith("vec3") && typeof value === "string" && value.length === 9) {
          throw new Error(`Uniform \`${name}\` is a vec3, so its colour cannot have alpha.`);
        }
      }
      return {
        type: "wgsl_shader",
        params: {
          shader: source,
          uniforms: Object.fromEntries(
            Object.entries(values).map(([name, value]) => [
              name,
              Array.isArray(value) ? value.map(lowerExpr) : lowerExpr(value),
            ]),
          ) as ShaderUniforms,
        },
      };
    },
  };
}

function uniformDeclarations(uniforms: WgslUniforms): string {
  const fields = Object.entries(uniforms);
  if (fields.length === 0) {
    return "";
  }

  const members = fields.map(([name, type]) => `    ${name}: ${type},\n`).join("");
  return `struct Uniforms {\n${members}}\n@group(0) @binding(1) var<uniform> uniforms: Uniforms;\n`;
}

function lowerExpr<T>(value: T): T | string {
  return value instanceof Expr ? value.toString() : value;
}
