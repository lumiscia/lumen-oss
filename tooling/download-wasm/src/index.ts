#!/usr/bin/env node

import { createHash } from "node:crypto";
import { createWriteStream } from "node:fs";
import { copyFile, mkdir, readFile, rename, rm, stat, writeFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { Readable } from "node:stream";
import { pipeline } from "node:stream/promises";
import type { ReadableStream as NodeReadableStream } from "node:stream/web";

interface CliOptions {
  baseUrl: string;
  repository: string;
  version: string;
  outputDir: string;
  versionFile: string;
  checksumOutput: string | null;
  targets: readonly BindingTarget[];
}

const DEFAULT_OUTPUT_DIR = "packages/lumen-bindings/src";
const DEFAULT_VERSION_FILE = "lumen-wasm.version.json";
const DEFAULT_TARGETS = ["bundler", "browser", "node", "no-modules"] as const;
const DEFAULT_REPOSITORY = "lumiscia/lumen";

type BindingTarget = (typeof DEFAULT_TARGETS)[number];

const targetFiles: Record<BindingTarget, readonly string[]> = {
  browser: ["lumen_wasm.d.ts", "lumen_wasm.js", "lumen_wasm_bg.wasm"],
  bundler: [
    "lumen_wasm.d.ts",
    "lumen_wasm.js",
    "lumen_wasm_bg.js",
    "lumen_wasm_bg.wasm",
    "lumen_wasm_bg.wasm.d.ts",
  ],
  node: ["lumen_wasm.d.ts", "lumen_wasm.js", "lumen_wasm_bg.wasm"],
  "no-modules": ["lumen_wasm.js", "lumen_wasm_bg.wasm"],
};

async function main(): Promise<void> {
  const options = parseArgs(process.argv.slice(2));
  const version = await readVersionFile(options.versionFile, {
    baseUrl: options.baseUrl,
    repository: options.repository,
    version: options.version,
  });
  const downloaded: Array<{ path: string; sha256: string; bytes: number }> = [];
  const wasmUrl = buildDownloadUrl(version.baseUrl, version.version, "lumen_wasm_bg.wasm");
  const wasmOutput = join(options.outputDir, ".cache", version.version, "lumen_wasm_bg.wasm");

  console.log(`Downloading ${wasmUrl}`);
  console.log(`Writing ${wasmOutput}`);
  const wasm = await downloadFile(wasmUrl, wasmOutput);
  if (version.wasmSha256 && wasm.sha256 !== version.wasmSha256) {
    await rm(wasmOutput, { force: true });
    throw new Error(
      `Checksum mismatch for lumen_wasm_bg.wasm: expected ${version.wasmSha256}, got ${wasm.sha256}`,
    );
  }
  downloaded.push({ path: "lumen_wasm_bg.wasm", ...wasm });

  for (const target of options.targets) {
    for (const fileName of targetFiles[target]) {
      if (fileName === "lumen_wasm_bg.wasm") {
        await copyDownloadedFile(wasmOutput, join(options.outputDir, target, fileName));
        downloaded.push({ bytes: wasm.bytes, path: `${target}/${fileName}`, sha256: wasm.sha256 });
        continue;
      }

      const url = buildDownloadUrl(version.baseUrl, version.version, `${target}/${fileName}`);
      const output = join(options.outputDir, target, fileName);

      console.log(`Downloading ${url}`);
      console.log(`Writing ${output}`);
      downloaded.push({ path: `${target}/${fileName}`, ...(await downloadFile(url, output)) });
    }
  }

  if (options.checksumOutput) {
    await mkdir(dirname(options.checksumOutput), { recursive: true });
    await writeFile(
      options.checksumOutput,
      downloaded.map((result) => `${result.sha256}  ${result.path}`).join("\n") + "\n",
    );
  }

  const totalBytes = downloaded.reduce((sum, result) => sum + result.bytes, 0);
  console.log(`Downloaded ${downloaded.length} files (${totalBytes} bytes)`);
}

async function copyDownloadedFile(source: string, output: string): Promise<void> {
  await mkdir(dirname(output), { recursive: true });
  const temporaryOutput = `${output}.tmp-${process.pid}`;
  try {
    await copyFile(source, temporaryOutput);
    await rename(temporaryOutput, output);
  } catch (error) {
    await rm(temporaryOutput, { force: true });
    throw error;
  }
}

function parseArgs(args: string[]): CliOptions {
  let baseUrl = process.env.LUMEN_WASM_BASE_URL ?? "";
  let repository = process.env.LUMEN_WASM_GITHUB_REPOSITORY ?? DEFAULT_REPOSITORY;
  let version = process.env.LUMEN_WASM_VERSION ?? "";
  let outputDir = process.env.LUMEN_BINDINGS_OUTPUT_DIR ?? DEFAULT_OUTPUT_DIR;
  let versionFile = process.env.LUMEN_WASM_VERSION_FILE ?? DEFAULT_VERSION_FILE;
  let checksumOutput = process.env.LUMEN_WASM_SHA256_OUTPUT ?? null;
  let targets = parseTargets(process.env.LUMEN_BINDINGS_TARGETS);

  for (let index = 0; index < args.length; index += 1) {
    const arg = args[index];

    switch (arg) {
      case "--base-url":
        baseUrl = readValue(args, ++index, arg);
        break;
      case "--github-repository":
      case "--repository":
        repository = readValue(args, ++index, arg);
        break;
      case "--version":
        version = readValue(args, ++index, arg);
        break;
      case "--output-dir":
        outputDir = readValue(args, ++index, arg);
        break;
      case "--version-file":
        versionFile = readValue(args, ++index, arg);
        break;
      case "--sha256-output":
        checksumOutput = readValue(args, ++index, arg);
        break;
      case "--targets":
        targets = parseTargets(readValue(args, ++index, arg));
        break;
      case "--help":
      case "-h":
        printHelp();
        process.exit(0);
      default:
        throw new Error(`Unknown argument: ${arg}`);
    }
  }

  return {
    baseUrl,
    repository,
    version,
    outputDir: resolve(outputDir),
    versionFile: resolve(versionFile),
    checksumOutput: checksumOutput ? resolve(checksumOutput) : null,
    targets,
  };
}

async function readVersionFile(
  path: string,
  overrides: { baseUrl: string; repository: string; version: string },
): Promise<{ baseUrl: string; version: string; wasmSha256: string | null }> {
  const file = await readFile(path, "utf8").catch((error: unknown) => {
    if (overrides.baseUrl && overrides.version) {
      return null;
    }

    throw error;
  });
  const parsed = file === null ? {} : (JSON.parse(file) as Partial<WasmVersionFile>);
  const version = overrides.version || parsed.version;
  const repository = overrides.repository || parsed.githubRepository || DEFAULT_REPOSITORY;
  const baseUrl =
    overrides.baseUrl ||
    parsed.baseUrl ||
    (version ? `https://github.com/${repository}/releases/download` : undefined);

  if (!baseUrl) {
    throw new Error(
      `Missing baseUrl in ${path}, --base-url, or LUMEN_WASM_BASE_URL, and no release version was provided`,
    );
  }
  if (!version) {
    throw new Error(`Missing version in ${path}, --version, or LUMEN_WASM_VERSION`);
  }

  return { baseUrl, version, wasmSha256: parsed.wasmSha256 ?? null };
}

interface WasmVersionFile {
  readonly baseUrl: string;
  readonly githubRepository?: string;
  readonly gitSha?: string;
  readonly manifestUrl?: string;
  readonly release?: string;
  readonly version: string;
  readonly wasmSha256?: string;
}

function parseTargets(value: string | undefined): readonly BindingTarget[] {
  if (!value) {
    return DEFAULT_TARGETS;
  }

  const targets: BindingTarget[] = [];
  const inputTargets = value
    .split(",")
    .map((target) => target.trim())
    .filter(Boolean);

  for (const target of inputTargets) {
    if (!isBindingTarget(target)) {
      throw new Error(`Unknown binding target: ${target}`);
    }
    targets.push(target);
  }

  return targets;
}

function isBindingTarget(value: string): value is BindingTarget {
  return (DEFAULT_TARGETS as readonly string[]).includes(value);
}

function readValue(args: string[], index: number, flag: string): string {
  const value = args[index];
  if (!value) {
    throw new Error(`Missing value for ${flag}`);
  }
  return value;
}

function buildDownloadUrl(baseUrl: string, version: string, fileName: string): string {
  const url = new URL(
    `${trimSlashes(version)}/${trimSlashes(fileName)}`,
    ensureTrailingSlash(baseUrl),
  );
  return url.toString();
}

function ensureTrailingSlash(value: string): string {
  return value.endsWith("/") ? value : `${value}/`;
}

function trimSlashes(value: string): string {
  return value.replace(/^\/+|\/+$/g, "");
}

async function downloadFile(
  url: string,
  output: string,
): Promise<{ bytes: number; sha256: string }> {
  const response = await fetch(url);
  if (!response.ok) {
    throw new Error(`Download failed with ${response.status} ${response.statusText}`);
  }

  if (!response.body) {
    throw new Error("Download response did not include a body");
  }

  await mkdir(dirname(output), { recursive: true });

  const temporaryOutput = `${output}.tmp-${process.pid}`;
  const hash = createHash("sha256");
  let bytes = 0;

  try {
    const source = Readable.fromWeb(response.body as NodeReadableStream<Uint8Array>);
    source.on("data", (chunk: Buffer) => {
      bytes += chunk.byteLength;
      hash.update(chunk);
    });

    await pipeline(source, createWriteStream(temporaryOutput, { flags: "wx" }));
    await rename(temporaryOutput, output);
  } catch (error) {
    await rm(temporaryOutput, { force: true });
    throw error;
  }

  const outputStat = await stat(output);
  return {
    bytes: outputStat.size || bytes,
    sha256: hash.digest("hex"),
  };
}

function printHelp(): void {
  console.log(`download-lumen-wasm

Downloads the Lumen WASM binding files from GitHub Releases or a compatible static host.

Options:
  --base-url <url>       Artifact base URL, or LUMEN_WASM_BASE_URL
  --repository <repo>    GitHub repository for release downloads (default: ${DEFAULT_REPOSITORY})
  --version <version>    Version path to fetch, or LUMEN_WASM_VERSION
  --version-file <path>  Version metadata file, or LUMEN_WASM_VERSION_FILE (default: ${DEFAULT_VERSION_FILE})
  --targets <targets>    Comma-separated targets (default: ${DEFAULT_TARGETS.join(",")})
  --output-dir <path>    Output directory, or LUMEN_BINDINGS_OUTPUT_DIR (default: ${DEFAULT_OUTPUT_DIR})
  --sha256-output <path> Write the downloaded SHA-256 to a file
`);
}

main().catch((error: unknown) => {
  console.error(error instanceof Error ? error.message : error);
  process.exitCode = 1;
});
