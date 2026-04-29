import { readFile } from "node:fs/promises";

export interface MetaManifest {
  readonly schemaVersion: number;
  readonly nodeKinds: readonly string[];
  readonly nodeSpecs: Readonly<Record<string, NodeSpec>>;
}

export interface NodeSpec {
  readonly kind: string;
  readonly label: string;
  readonly description: string;
  readonly category: string;
  readonly inputs?: readonly NodePortSpec[];
  readonly outputs?: readonly NodePortSpec[];
  readonly properties?: readonly NodePropertySpec[];
  readonly defaultProperties?: Readonly<Record<string, unknown>>;
}

export interface NodePortSpec {
  readonly name: string;
  readonly kind: string;
  readonly optional?: boolean;
  readonly variadic?: boolean;
}

export interface NodePropertySpec {
  readonly name: string;
  readonly kind: string;
  readonly defaultValue?: unknown;
}

export async function readMetaManifest(path: string): Promise<MetaManifest> {
  const raw = await readFile(path, "utf8");
  return JSON.parse(raw) as MetaManifest;
}
