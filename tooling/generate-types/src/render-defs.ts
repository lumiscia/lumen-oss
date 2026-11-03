export interface JsonSchema {
  readonly [key: string]: unknown;
}

const expressionRef = "#/$defs/expression";
const hexColorPattern = "^#([0-9a-fA-F]{6}|[0-9a-fA-F]{8})$";
const identifier = /^[A-Za-z_$][A-Za-z0-9_$]*$/u;

/** Keywords that decide the TypeScript type. */
const typeKeywords = new Set([
  "$ref",
  "const",
  "enum",
  "anyOf",
  "oneOf",
  "type",
  "pattern",
  "items",
  "prefixItems",
  "minItems",
  "maxItems",
  "properties",
  "required",
  "additionalProperties",
]);

/**
 * Keywords with no effect on the TypeScript type: annotations, and value checks that a type
 * can't express. Any other keyword is an error, so a new one can't be dropped unnoticed.
 */
const ignoredKeywords = new Set([
  "$schema",
  "$defs",
  "title",
  "description",
  "default",
  "minimum",
  "maximum",
  "exclusiveMinimum",
  "minLength",
  "maxLength",
  "propertyNames",
  "not",
]);

export interface TypeContext {
  /** The TypeScript name of each `$ref` target. */
  readonly names: ReadonlyMap<string, string>;
  /** `export type` declarations by name. */
  readonly declarations: Map<string, string>;
  /** Every type name the rendered types mention. */
  readonly used: Set<string>;
}

export const object = (value: unknown): JsonSchema =>
  value !== null && typeof value === "object" && !Array.isArray(value) ? (value as JsonSchema) : {};

export const array = (value: unknown): readonly unknown[] => (Array.isArray(value) ? value : []);

const pascalCase = (value: string): string => `${value[0]?.toUpperCase() ?? ""}${value.slice(1)}`;

export const docComment = (description: unknown, indent: string): string => {
  if (typeof description !== "string" || description === "") return "";
  const lines = description.split("\n");
  return lines.length === 1
    ? `${indent}/** ${lines[0]} */\n`
    : `${indent}/**\n${lines.map((line) => `${indent} * ${line}`).join("\n")}\n${indent} */\n`;
};

const parenthesize = (type: string): string => (/[|&]/u.test(type) ? `(${type})` : type);

const propertyKey = (key: string): string => (identifier.test(key) ? key : JSON.stringify(key));

/** The type context for `defs`; `names` adds the names of other `$ref` targets. */
export function createTypeContext(
  defs: Readonly<Record<string, JsonSchema>>,
  names: Iterable<readonly [string, string]> = [],
): TypeContext {
  const defNames = Object.entries(defs).map(([key, def]): [string, string] => {
    const name = def["x-lumen-type"];
    return [`#/$defs/${key}`, typeof name === "string" ? name : pascalCase(key)];
  });
  return {
    names: new Map([...defNames, ...names]),
    declarations: new Map(),
    used: new Set(),
  };
}

/** Declares a type; declaring a name twice must declare the same type. */
export function declare(context: TypeContext, name: string, declaration: string): void {
  const existing = context.declarations.get(name);
  if (existing !== undefined && existing !== declaration) {
    throw new Error(`conflicting declarations of \`${name}\``);
  }
  context.declarations.set(name, declaration);
}

/** `export type` for `schema`, with its description as JSDoc when `documented`. */
export function typeDeclaration(
  name: string,
  schema: JsonSchema,
  context: TypeContext,
  documented: boolean,
): string {
  const doc = documented ? docComment(schema.description, "") : "";
  return `${doc}export type ${name} = ${typeBody(schema, context, "")};`;
}

/** Declares every `$defs` entry that carries an `x-lumen-type`. */
export function declareDefs(
  context: TypeContext,
  defs: Readonly<Record<string, JsonSchema>>,
): void {
  for (const def of Object.values(defs)) {
    const name = def["x-lumen-type"];
    if (typeof name === "string") declare(context, name, typeDeclaration(name, def, context, true));
  }
}

/**
 * The type of `schema`. A schema with an `x-lumen-type` is declared under that name, so every
 * schema naming the same type shares one declaration. Expression references, hex colour
 * patterns and `anyOf` with an expression become `ExpressionString`, `HexColor` and
 * `ExpressionValue<T>`; arrays of at most 4 items become tuples.
 */
export function schemaType(schema: JsonSchema, context: TypeContext, indent: string): string {
  const name = schema["x-lumen-type"];
  if (typeof name !== "string") return typeBody(schema, context, indent);
  declare(context, name, typeDeclaration(name, schema, context, false));
  context.used.add(name);
  return name;
}

function assertSupportedKeywords(schema: JsonSchema): void {
  for (const keyword of Object.keys(schema)) {
    if (
      !typeKeywords.has(keyword) &&
      !ignoredKeywords.has(keyword) &&
      !keyword.startsWith("x-lumen-")
    ) {
      throw new Error(`unsupported schema keyword \`${keyword}\` in ${JSON.stringify(schema)}`);
    }
  }
}

function typeBody(schema: JsonSchema, context: TypeContext, indent: string): string {
  assertSupportedKeywords(schema);
  const type = (value: unknown): string => schemaType(object(value), context, indent);
  const reference = (name: string): string => {
    context.used.add(name);
    return name;
  };
  if (typeof schema.$ref === "string") {
    if (schema.$ref === expressionRef) return reference("ExpressionString");
    const name = context.names.get(schema.$ref);
    if (name === undefined) throw new Error(`unknown $ref \`${schema.$ref}\``);
    return reference(name);
  }
  if (schema.const !== undefined) return JSON.stringify(schema.const);
  if (Array.isArray(schema.enum))
    return schema.enum.map((value) => JSON.stringify(value)).join(" | ");
  const branches = array(schema.anyOf ?? schema.oneOf);
  if (branches.length > 0) {
    for (const branch of branches) assertSupportedKeywords(object(branch));
    const isExpression = (branch: unknown): boolean => object(branch).$ref === expressionRef;
    const literal = branches
      .filter((branch) => !isExpression(branch))
      .map(type)
      .join(" | ");
    if (!branches.some(isExpression)) return literal;
    return `${reference("ExpressionValue")}<${literal}>`;
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
      return schema.pattern === hexColorPattern ? reference("HexColor") : "string";
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
      return objectType(schema, context, indent);
    default:
      throw new Error(`unsupported schema ${JSON.stringify(schema)}`);
  }
}

/**
 * Optional properties unless `required`. `additionalProperties: true` adds an index signature,
 * and a schema there makes a record, keyed by `x-lumen-keyType` when it has one.
 */
function objectType(schema: JsonSchema, context: TypeContext, indent: string): string {
  const properties = Object.entries(object(schema.properties));
  const additional = schema.additionalProperties;
  const additionalIsSchema = additional !== null && typeof additional === "object";
  const open = additional === undefined || additional === true;
  if (properties.length === 0 && (additionalIsSchema || open)) {
    const keys = schema["x-lumen-keyType"];
    const values = additionalIsSchema ? schemaType(object(additional), context, indent) : "unknown";
    return `Readonly<Record<${typeof keys === "string" ? keys : "string"}, ${values}>>`;
  }
  if (additionalIsSchema) {
    throw new Error(`unsupported schema with properties and additionalProperties schema`);
  }
  if (properties.length === 0) return "Readonly<Record<string, never>>";
  const required = new Set(array(schema.required));
  const fieldIndent = `${indent}  `;
  const fields = properties.map(([key, value]) => {
    const field = object(value);
    const optional = required.has(key) ? "" : "?";
    return `${docComment(field.description, fieldIndent)}${fieldIndent}readonly ${propertyKey(key)}${optional}: ${schemaType(field, context, fieldIndent)};`;
  });
  if (open) fields.push(`${fieldIndent}readonly [key: string]: unknown;`);
  return `{\n${fields.join("\n")}\n${indent}}`;
}
