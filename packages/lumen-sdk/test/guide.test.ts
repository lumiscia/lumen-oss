import { readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

import ts from "typescript";
import { expect, test } from "vitest";

import * as sdk from "../src/index.js";

// docs/3d.md pairs each `json` block with the `ts` block after it: the TypeScript builds the
// composition and the JSON is its nodes and connections. Every `ts` block must type-check and
// define `composition`. `UPDATE_GUIDE=1` rewrites the JSON blocks from the TypeScript ones, on one
// line each for the formatter to lay out (`just update-3d-guide` does both);
// `crates/cli/tests/docs.rs` checks that the engine accepts them.
const guidePath = fileURLToPath(new URL("../../../docs/3d.md", import.meta.url));
const sdkPath = fileURLToPath(new URL("../src/index.ts", import.meta.url));
const baseConfigPath = fileURLToPath(new URL("../../../tsconfig.base.json", import.meta.url));

interface Block {
  readonly lang: string;
  readonly code: string;
  readonly start: number;
  readonly end: number;
}

const markdown = readFileSync(guidePath, "utf8");
const blocks: Block[] = [...markdown.matchAll(/^```(\w+)\n([\s\S]*?)^```$/gmu)].map((match) => ({
  lang: match[1] ?? "",
  code: match[2] ?? "",
  start: match.index + match[0].indexOf("\n") + 1,
  end: match.index + match[0].length - 3,
}));
const tsBlocks = blocks.filter((block) => block.lang === "ts");

function typeErrors(): string[] {
  const base = ts.readConfigFile(baseConfigPath, ts.sys.readFile).config as {
    compilerOptions: object;
  };
  const { options } = ts.parseJsonConfigFileContent(
    { compilerOptions: { ...base.compilerOptions, noEmit: true, types: [] } },
    ts.sys,
    fileURLToPath(new URL("..", import.meta.url)),
  );
  options.paths = { "@lumiscia/lumen-sdk": [sdkPath] };
  const virtualDir = fileURLToPath(new URL("./", import.meta.url));
  const files = new Map(
    tsBlocks.map((block, index) => [`${virtualDir}guide-${index}.ts`, block.code]),
  );
  const host = ts.createCompilerHost(options);
  const { getSourceFile, fileExists, readFile } = host;
  host.fileExists = (name) => files.has(name) || fileExists(name);
  host.readFile = (name) => files.get(name) ?? readFile(name);
  host.getSourceFile = (name, languageVersion, ...rest) => {
    const text = files.get(name);
    return text === undefined
      ? getSourceFile(name, languageVersion, ...rest)
      : ts.createSourceFile(name, text, languageVersion, true);
  };
  const program = ts.createProgram([...files.keys()], options, host);
  return ts.getPreEmitDiagnostics(program).map((diagnostic) => {
    const file = diagnostic.file?.fileName.replace(virtualDir, "") ?? "";
    return `${file}: ${ts.flattenDiagnosticMessageText(diagnostic.messageText, "\n")}`;
  });
}

function compose(code: string): { nodes: object[]; connections: object[] } {
  const { outputText } = ts.transpileModule(code, {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 },
  });
  const exports: { composition?: unknown } = {};
  new Function("require", "exports", `${outputText}\nexports.composition = composition;`)(
    () => sdk,
    exports,
  );
  const { nodes, connections } = JSON.parse(JSON.stringify(exports.composition)) as {
    nodes: { id: number; type: string; params?: object }[];
    connections: object[];
  };
  return {
    nodes: nodes.map(({ id, type, params }) => ({ id, type, ...(params ? { params } : {}) })),
    connections,
  };
}

test("every TypeScript block in the 3D guide type-checks", () => {
  expect(tsBlocks.length).toBeGreaterThan(0);
  expect(typeErrors()).toEqual([]);
});

test("every JSON block in the 3D guide is the composition the TypeScript block after it builds", () => {
  const pairs = blocks.flatMap((block, index) => {
    if (block.lang !== "json") return [];
    const next = blocks[index + 1];
    if (next?.lang !== "ts")
      throw new Error(`the JSON block at offset ${block.start} has no ts block after it`);
    return [{ json: block, composition: compose(next.code) }];
  });
  expect(pairs.length).toBeGreaterThan(0);

  if (process.env.UPDATE_GUIDE === "1") {
    let updated = markdown;
    for (const { json, composition } of [...pairs].reverse()) {
      updated = `${updated.slice(0, json.start)}${JSON.stringify(composition)}\n${updated.slice(json.end)}`;
    }
    writeFileSync(guidePath, updated);
    return;
  }
  for (const { json, composition } of pairs) {
    expect(JSON.parse(json.code), "run `just update-3d-guide` after changing the guide").toEqual(
      composition,
    );
  }
});
