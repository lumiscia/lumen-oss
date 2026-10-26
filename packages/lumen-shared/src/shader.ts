import type { ShaderUniforms } from "@lumiscia/lumen-types";

import { Expr } from "./expression.js";
import type { WgslShader, WgslShaderOptions, WgslUniformType, WgslUniforms } from "./types.js";

const uniformTypes: Readonly<Record<WgslUniformType, true>> = {
  f32: true,
  i32: true,
  u32: true,
  "vec2<f32>": true,
  "vec2<i32>": true,
  "vec2<u32>": true,
  "vec3<f32>": true,
  "vec3<i32>": true,
  "vec3<u32>": true,
  "vec4<f32>": true,
  "vec4<i32>": true,
  "vec4<u32>": true,
};

const identifier = /^[A-Za-z_][A-Za-z0-9_]*$/u;

// WGSL keywords and reserved words, which cannot name a struct member.
const reserved = new Set(
  `alias break case const const_assert continue continuing default diagnostic discard else enable
  false fn for if let loop override requires return struct switch true var while NULL Self abstract
  active alignas alignof as asm asm_fragment async attribute auto await become cast catch class
  co_await co_return co_yield coherent column_major common compile compile_fragment concept
  const_cast consteval constexpr constinit crate debugger decltype delete demote demote_to_helper do
  dynamic_cast enum explicit export extends extern external fallthrough filter final finally friend
  from fxgroup get goto groupshared highp impl implements import inline instanceof interface layout
  lowp macro macro_rules match mediump meta mod module move mut mutable namespace new nil noexcept
  noinline nointerpolation non_coherent noncoherent noperspective null nullptr of operator package
  packoffset partition pass patch pixelfragment precise precision premerge priv protected pub public
  readonly ref regardless register reinterpret_cast require resource restrict self set shared sizeof
  smooth snorm static static_assert static_cast std subroutine super target template this
  thread_local throw trait try type typedef typeid typename typeof union unless unorm unsafe unsized
  use using varying virtual volatile wgsl where with writeonly yield`.split(/\s+/u),
);

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

  for (const [name, type] of fields) {
    // WGSL reserves `_` and identifiers starting with `__`.
    if (!identifier.test(name) || name === "_" || name.startsWith("__") || reserved.has(name)) {
      throw new Error(`Uniform name \`${name}\` is not a WGSL identifier.`);
    }
    if (!Object.hasOwn(uniformTypes, type)) {
      throw new Error(`Uniform \`${name}\` has unsupported type \`${type}\`.`);
    }
  }

  const members = fields.map(([name, type]) => `    ${name}: ${type},\n`).join("");
  return `struct Uniforms {\n${members}}\n@group(0) @binding(1) var<uniform> uniforms: Uniforms;\n`;
}

function lowerExpr<T>(value: T): T | string {
  return value instanceof Expr ? value.toString() : value;
}
