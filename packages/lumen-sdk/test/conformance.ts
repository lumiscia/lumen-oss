import { readFile } from "node:fs/promises";

import { apiOperations, type ApiOperationId } from "@lumiscia/lumen-types";
import { Ajv2020, type ValidateFunction } from "ajv/dist/2020.js";

const definitions = new URL("../../../definitions/", import.meta.url);
const base = "https://lumen.invalid/";
const schemas = `${base}openapi.json#/components/schemas/`;

/**
 * Where a host of the API differs from `definitions/openapi.json` on purpose. Everything a host
 * does that neither the document nor its profile describes is a violation.
 */
export interface ConformanceProfile {
  /** Operations the host doesn't have. Tests that need one are skipped. */
  readonly unsupported?: readonly ApiOperationId[];
  /** Values the host adds to a schema the document marks `x-lumen-open`, by schema name. */
  readonly extraValues?: Readonly<Record<string, readonly string[]>>;
  /** Error statuses the host answers on any operation, such as `429`, with the API's error body. */
  readonly extraStatuses?: readonly number[];
  /** Error codes of the host's own, which it may answer with any error status. */
  readonly extraCodes?: readonly string[];
}

type Json = Record<string, unknown>;

interface Described {
  readonly id: ApiOperationId;
  readonly segments: readonly string[];
  readonly method: string;
  readonly responses: Readonly<Record<string, Json>>;
}

/** Checks every response of a host against the OpenAPI document, through the `fetch` it wraps. */
export class Conformance {
  /** What the host did that the document and the profile don't describe. */
  readonly violations: string[] = [];
  readonly #ajv: Ajv2020;
  readonly #operations: readonly Described[];
  readonly #profile: ConformanceProfile;
  readonly #basePath: string;

  private constructor(ajv: Ajv2020, document: Json, profile: ConformanceProfile, baseUrl: URL) {
    this.#ajv = ajv;
    this.#profile = profile;
    this.#basePath = baseUrl.pathname.replace(/\/$/, "");
    const paths = document.paths as Record<string, Record<string, Json>>;
    this.#operations = Object.entries(apiOperations).map(([id, operation]) => ({
      id: id as ApiOperationId,
      segments: operation.path.split("/"),
      method: operation.method,
      responses: paths[operation.path]?.[operation.method.toLowerCase()]?.responses as Record<
        string,
        Json
      >,
    }));
  }

  /** Loads the definitions and applies `profile`'s extra values to the schemas that allow them. */
  static async load(baseUrl: string, profile: ConformanceProfile = {}): Promise<Conformance> {
    const read = async (name: string) =>
      JSON.parse(await readFile(new URL(name, definitions), "utf8")) as Json;
    const document = await read("openapi.json");
    const components = (document.components as Json).schemas as Record<string, Json>;
    for (const [name, values] of Object.entries(profile.extraValues ?? {})) {
      const schema = components[name];
      if (schema?.["x-lumen-open"] !== true) {
        throw new Error(
          `The profile adds values to \`${name}\`, which the API doesn't leave open.`,
        );
      }
      if (Array.isArray(schema.enum)) {
        schema.enum.push(...values);
      } else if (Array.isArray(schema.oneOf)) {
        schema.oneOf.push(...values.map((value) => ({ const: value })));
      } else {
        throw new Error(`\`${name}\` is not a list of values.`);
      }
    }
    // The definitions carry `x-lumen-*` keywords, and the document is not itself a schema.
    const ajv = new Ajv2020({ strict: false, validateSchema: false });
    ajv.addSchema(document, `${base}openapi.json`);
    ajv.addSchema(await read("runtime.schema.json"), `${base}runtime.schema.json`);
    ajv.addSchema(await read("composition.schema.json"), `${base}composition.schema.json`);
    return new Conformance(ajv, document, profile, new URL(baseUrl));
  }

  supports(...operations: ApiOperationId[]): boolean {
    return operations.every((id) => !this.#profile.unsupported?.includes(id));
  }

  /** `fetch`, with each response to an operation of the API checked as it passes. */
  readonly fetch: typeof fetch = async (input, init) => {
    const response = await fetch(input, init);
    const request = input instanceof Request ? input : undefined;
    const url = new URL(request?.url ?? String(input));
    const method = (init?.method ?? request?.method ?? "GET").toUpperCase();
    const operation = this.#operation(method, url.pathname);
    if (operation) {
      await this.#check(operation, response.clone());
    }
    return response;
  };

  #operation(method: string, pathname: string): Described | undefined {
    if (!pathname.startsWith(this.#basePath)) {
      return undefined;
    }
    const segments = pathname.slice(this.#basePath.length).split("/");
    return this.#operations.find(
      (operation) =>
        operation.method === method &&
        operation.segments.length === segments.length &&
        operation.segments.every(
          (expected, index) =>
            (expected.startsWith("{") && segments[index] !== "") || expected === segments[index],
        ),
    );
  }

  async #check(operation: Described, response: Response): Promise<void> {
    const where = `${operation.id} answered ${response.status}`;
    if (!this.supports(operation.id)) {
      this.violations.push(`${where}, and the profile says the host doesn't have it`);
      return;
    }
    const described = operation.responses[String(response.status)];
    const extra = this.#profile.extraStatuses?.includes(response.status) === true;
    if (!described && !extra) {
      this.violations.push(`${where}, which the API doesn't describe for it`);
      return;
    }
    const content = (described?.content ?? {}) as Record<string, { schema?: { $ref?: string } }>;
    const ref =
      extra && !described ? `${schemas}ErrorBody` : content["application/json"]?.schema?.$ref;
    if (ref === undefined) {
      return;
    }
    const name = ref.slice(ref.lastIndexOf("/") + 1);
    const body: unknown = await response.json().catch(() => undefined);
    const validate = this.#validator(name);
    if (!validate(body)) {
      const errors = (validate.errors ?? []).map(
        (error) => `${error.instancePath} ${error.message}`,
      );
      this.violations.push(`${where} with a body that is not a \`${name}\`: ${errors.join("; ")}`);
      return;
    }
    const codes = described?.["x-lumen-codes"];
    const code = (body as { code?: unknown }).code;
    const known = this.#profile.extraCodes?.includes(String(code)) === true;
    if (Array.isArray(codes) && !codes.includes("*") && !codes.includes(code) && !known) {
      this.violations.push(
        `${where} with the code \`${String(code)}\`, not one of ${codes.join(", ")}`,
      );
    }
  }

  #validator(name: string): ValidateFunction {
    const validate = this.#ajv.getSchema(`${schemas}${name}`);
    if (!validate) {
      throw new Error(`The API has no schema \`${name}\`.`);
    }
    return validate;
  }
}
