import { readFile, writeFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const dirnamePath = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(dirnamePath, "../../..");
const packageJsonPath = resolve(repoRoot, "packages/lumen-types/package.json");
const cargoTomlPath = resolve(repoRoot, "Cargo.toml");

interface PackageJson {
  readonly version?: unknown;
}

const packageJson = JSON.parse(await readFile(packageJsonPath, "utf8")) as PackageJson;
const version = packageJson.version;

if (typeof version !== "string" || !/^\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?$/.test(version)) {
  throw new Error(`Expected ${packageJsonPath} to contain a semver version, got ${version}`);
}

const cargoToml = await readFile(cargoTomlPath, "utf8");
const { found, contents } = syncWorkspacePackageVersion(cargoToml, version);

if (!found) {
  throw new Error(`Could not find [workspace.package] version in ${cargoTomlPath}`);
}

if (contents !== cargoToml) {
  await writeFile(cargoTomlPath, contents);
}
console.log(`Synced Cargo workspace version to ${version}`);

interface SyncResult {
  readonly found: boolean;
  readonly contents: string;
}

function syncWorkspacePackageVersion(cargoToml: string, version: string): SyncResult {
  const lines = cargoToml.split(/\r?\n/);
  let workspacePackageLine: number | undefined;
  let inWorkspacePackage = false;

  for (const [index, line] of lines.entries()) {
    const trimmed = line.trim();
    const tableMatch = /^\[+[^\]]+\]+$/.test(trimmed);

    if (tableMatch) {
      inWorkspacePackage = trimmed === "[workspace.package]";
      if (inWorkspacePackage) {
        workspacePackageLine = index;
      }
      continue;
    }

    if (inWorkspacePackage && /^\s*version\s*=/.test(line)) {
      lines[index] = line.replace(/^\s*version\s*=.*$/, `version = "${version}"`);
      return { found: true, contents: lines.join("\n") };
    }
  }

  if (workspacePackageLine !== undefined) {
    lines.splice(workspacePackageLine + 1, 0, `version = "${version}"`);
    return { found: true, contents: lines.join("\n") };
  }

  return { found: false, contents: cargoToml };
}
