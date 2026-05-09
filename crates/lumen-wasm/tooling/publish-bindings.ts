import { appendFile, mkdir, rm, writeFile } from "node:fs/promises";
import { basename, join, resolve } from "node:path";

type BunFile = {
  readonly size: number;
  arrayBuffer(): Promise<ArrayBuffer>;
};

type ArtifactFile = {
  path: string;
  rel: string;
  sha256: string;
  size: number;
};

declare const Bun: {
  $: {
    (strings: TemplateStringsArray, ...values: unknown[]): Promise<unknown>;
    cwd(path: string): void;
  };
  Glob: new (pattern: string) => {
    scan(options: { cwd: string; onlyFiles: boolean }): AsyncIterable<string>;
  };
  file(path: string): BunFile;
  write(path: string, data: BunFile): Promise<number>;
};

const $ = Bun.$;
const repoRoot = resolve(import.meta.dirname, "../../..");
const options = parseArgs();
const bindingsDir = options.bindingsDir;
const outDir = options.outDir;
const wasmName = "lumen_wasm";
const sharedWasmName = `${wasmName}_bg.wasm`;
const gitSha = process.env.GITHUB_SHA ?? "local";
const releaseTag = process.env.RELEASE_TAG;
const repository = process.env.GITHUB_REPOSITORY ?? "lumiscia/lumen";
const baseUrl = `https://github.com/${repository}/releases/download`;
const shaKey = `lumen/sha-${gitSha}`;
const targets = ["bundler", "browser", "node", "no-modules"] as const;

$.cwd(repoRoot);

await prepareArtifacts();
const files = await artifactFiles();
const wasmSha256 = requiredFile(files, sharedWasmName).sha256;

await writeManifest(files);

if (releaseTag) {
  await writeReleasePointer(wasmSha256);
}

await writeGithubOutputs(wasmSha256);
await writeSummary(wasmSha256);

async function prepareArtifacts() {
  await rm(outDir, { recursive: true, force: true });
  await mkdir(outDir, { recursive: true });

  const wasmSources = await Promise.all(
    targets.map(async (target) => ({
      sha256: await hashFile(Bun.file(join(bindingsDir, target, sharedWasmName))),
      target,
    })),
  );
  const uniqueWasmHashes = new Set(wasmSources.map((entry) => entry.sha256));
  if (uniqueWasmHashes.size !== 1) {
    throw new Error(
      `Cannot publish bindings because generated wasm files differ:\n${wasmSources
        .map((entry) => `- ${entry.target}: ${entry.sha256}`)
        .join("\n")}`,
    );
  }

  await Bun.write(
    join(outDir, sharedWasmName),
    Bun.file(join(bindingsDir, "bundler", sharedWasmName)),
  );

  for (const target of targets) {
    const targetOut = join(outDir, target);
    await mkdir(targetOut, { recursive: true });
    for await (const source of new Bun.Glob("*").scan({
      cwd: join(bindingsDir, target),
      onlyFiles: true,
    })) {
      if (basename(source) === sharedWasmName) {
        continue;
      }
      await Bun.write(join(targetOut, source), Bun.file(join(bindingsDir, target, source)));
    }
  }
}

async function artifactFiles() {
  const files: ArtifactFile[] = [];
  for await (const file of new Bun.Glob("**/*").scan({ cwd: outDir, onlyFiles: true })) {
    if (file === "manifest.json" || file === "release-pointer.json") {
      continue;
    }
    const path = join(outDir, file);
    const bunFile = Bun.file(path);
    files.push({
      path,
      rel: file,
      sha256: await hashFile(bunFile),
      size: bunFile.size,
    });
  }

  files.sort((left, right) => left.rel.localeCompare(right.rel));
  await writeFile(
    join(outDir, "SHA256SUMS"),
    `${files.map((file) => `${file.sha256}  ${file.rel}`).join("\n")}\n`,
  );

  files.push({
    path: join(outDir, "SHA256SUMS"),
    rel: "SHA256SUMS",
    sha256: await hashFile(Bun.file(join(outDir, "SHA256SUMS"))),
    size: Bun.file(join(outDir, "SHA256SUMS")).size,
  });
  files.sort((left, right) => left.rel.localeCompare(right.rel));
  return files;
}

async function writeManifest(files: ArtifactFile[]) {
  const manifest = {
    package: "lumen-bindings",
    version: releaseTag ?? shaKey,
    gitSha,
    files: Object.fromEntries(
      files
        .filter((file) => file.rel !== "manifest.json")
        .map((file) => [file.rel, { sha256: file.sha256, size: file.size }]),
    ),
  };

  await writeJson(join(outDir, "manifest.json"), manifest);
}

async function writeReleasePointer(wasmSha256: string) {
  await writeJson(join(outDir, "release-pointer.json"), {
    baseUrl,
    version: releaseTag,
    release: releaseTag,
    gitSha,
    manifestUrl: `${baseUrl}/${releaseTag}/manifest.json`,
    wasmSha256,
  });
}

async function writeGithubOutputs(wasmSha256: string) {
  const output = process.env.GITHUB_OUTPUT;
  if (!output) {
    return;
  }

  await appendFile(output, `sha_key=${shaKey}\nwasm_sha256=${wasmSha256}\n`);
}

async function writeSummary(wasmSha256: string) {
  const summary = process.env.GITHUB_STEP_SUMMARY;
  if (!summary) {
    return;
  }

  await appendFile(
    summary,
    [
      "## Lumen WASM published",
      "",
      `- Release: \`${releaseTag ?? "local"}\``,
      `- WASM: \`${sharedWasmName}\``,
      `- WASM SHA-256: \`${wasmSha256}\``,
      "",
    ].join("\n"),
  );
}

function requiredFile(files: ArtifactFile[], rel: string) {
  const file = files.find((candidate) => candidate.rel === rel);
  if (!file) {
    throw new Error(`Missing artifact file: ${rel}`);
  }
  return file;
}

function parseArgs() {
  let bindingsDir: string | undefined;
  let outDir: string | undefined;
  const args = process.argv.slice(2);

  for (let index = 0; index < args.length; index += 1) {
    const arg = args[index];
    if (arg === "--bindings-dir") {
      bindingsDir = resolve(repoRoot, requiredValue(args, index));
      index += 1;
      continue;
    }
    if (arg === "--out-dir") {
      outDir = resolve(repoRoot, requiredValue(args, index));
      index += 1;
      continue;
    }
    if (arg === "--dry-run") continue;

    throw new Error(`Unknown argument: ${arg}`);
  }

  if (!bindingsDir) {
    throw new Error("Missing required --bindings-dir <path>.");
  }
  if (!outDir) {
    throw new Error("Missing required --out-dir <path>.");
  }

  return { bindingsDir, outDir };
}

function requiredValue(args: string[], index: number) {
  const value = args[index + 1];
  if (!value) {
    throw new Error(`Missing value for ${args[index]}.`);
  }
  return value;
}

async function hashFile(file: BunFile) {
  const bytes = await file.arrayBuffer();
  const digest = await crypto.subtle.digest("SHA-256", bytes);
  return [...new Uint8Array(digest)].map((byte) => byte.toString(16).padStart(2, "0")).join("");
}

async function writeJson(path: string, value: unknown) {
  await writeFile(path, `${JSON.stringify(value, null, 2)}\n`);
}
