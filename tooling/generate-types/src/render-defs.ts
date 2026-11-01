import type { JsonSchema } from "./definitions.js";

const expressionRef = "#/$defs/expression";
const hexColorPattern = "^#([0-9a-fA-F]{6}|[0-9a-fA-F]{8})$";

const pascalCase = (value: string): string => `${value[0]?.toUpperCase() ?? ""}${value.slice(1)}`;

const object = (value: unknown): JsonSchema =>
  value !== null && typeof value === "object" && !Array.isArray(value) ? (value as JsonSchema) : {};

const array = (value: unknown): readonly unknown[] => (Array.isArray(value) ? value : []);

const docComment = (description: unknown, indent: string): string => {
  if (typeof description !== "string" || description === "") return "";
  const lines = description.split("\n");
  return lines.length === 1
    ? `${indent}/** ${lines[0]} */\n`
    : `${indent}/**\n${lines.map((line) => `${indent} * ${line}`).join("\n")}\n${indent} */\n`;
};

const parenthesize = (type: string): string => (/[|&]/u.test(type) ? `(${type})` : type);

/** The TypeScript type names of `$defs`: their `x-lumen-type`, or the key in PascalCase. */
const defTypeNames = (defs: Readonly<Record<string, JsonSchema>>): Map<string, string> =>
  new Map(
    Object.entries(defs).map(([key, def]) => {
      const name = def["x-lumen-type"];
      return [`#/$defs/${key}`, typeof name === "string" ? name : pascalCase(key)];
    }),
  );

function schemaType(schema: JsonSchema, names: Map<string, string>, indent: string): string {
  const type = (value: unknown): string => schemaType(object(value), names, indent);
  if (typeof schema.$ref === "string") {
    if (schema.$ref === expressionRef) return "ExpressionString";
    const name = names.get(schema.$ref);
    if (name === undefined) throw new Error(`unknown $ref \`${schema.$ref}\``);
    return name;
  }
  if (schema.const !== undefined) return JSON.stringify(schema.const);
  if (Array.isArray(schema.enum))
    return schema.enum.map((value) => JSON.stringify(value)).join(" | ");
  const branches = array(schema.anyOf ?? schema.oneOf);
  if (branches.length > 0) {
    const isExpression = (branch: unknown): boolean => object(branch).$ref === expressionRef;
    const literal = branches
      .filter((branch) => !isExpression(branch))
      .map(type)
      .join(" | ");
    return branches.some(isExpression) ? `ExpressionValue<${literal}>` : literal;
  }
  switch (schema.type) {
    case "null":
      return "null";
    case "boolean":
      return "boolean";
    case "integer":
    case "number":
      return "number";
    case "string":
      return schema.pattern === hexColorPattern ? "HexColor" : "string";
    case "array": {
      const prefixItems = array(schema.prefixItems);
      if (prefixItems.length > 0) return `readonly [${prefixItems.map(type).join(", ")}]`;
      const item = type(schema.items);
      const { minItems, maxItems } = schema;
      if (typeof minItems === "number" && typeof maxItems === "number" && maxItems <= 4) {
        return Array.from({ length: maxItems - minItems + 1 }, (_, index) => {
          const items = Array.from({ length: minItems + index }, () => item);
          return `readonly [${items.join(", ")}]`;
        }).join(" | ");
      }
      return `readonly ${parenthesize(item)}[]`;
    }
    case "object":
      return objectType(schema, names, indent);
    default:
      throw new Error(`unsupported schema ${JSON.stringify(schema)}`);
  }
}

function objectType(schema: JsonSchema, names: Map<string, string>, indent: string): string {
  const properties = Object.entries(object(schema.properties));
  if (properties.length === 0 && schema.additionalProperties !== undefined) {
    const values = object(schema.additionalProperties);
    return `Readonly<Record<string, ${schemaType(values, names, indent)}>>`;
  }
  const required = new Set(array(schema.required));
  const fieldIndent = `${indent}  `;
  const fields = properties.map(([key, value]) => {
    const field = object(value);
    const optional = required.has(key) ? "" : "?";
    return `${docComment(field.description, fieldIndent)}${fieldIndent}readonly ${key}${optional}: ${schemaType(field, names, fieldIndent)};`;
  });
  return `{\n${fields.join("\n")}\n${indent}}`;
}

/**
 * `export type` declarations, keyed by name, for the `$defs` that carry an `x-lumen-type`.
 * Expression references, hex colour patterns and `anyOf` with an expression become
 * `ExpressionString`, `HexColor` and `ExpressionValue<T>`; arrays of at most 4 items become
 * tuples.
 */
export function renderDefTypes(defs: Readonly<Record<string, JsonSchema>>): Map<string, string> {
  const names = defTypeNames(defs);
  return new Map(
    Object.values(defs).flatMap((def) => {
      const name = def["x-lumen-type"];
      if (typeof name !== "string") return [];
      const declaration = `${docComment(def.description, "")}export type ${name} = ${schemaType(def, names, "")};`;
      return [[name, declaration] as const];
    }),
  );
}
