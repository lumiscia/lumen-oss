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
const contents = syncWorkspaceVersions(cargoToml, version);

if (contents !== cargoToml) {
  await writeFile(cargoTomlPath, contents);
}
console.log(`Synced Cargo workspace version to ${version}`);

// Sets `version` in [workspace.package] and on each internal crate in [workspace.dependencies].
function syncWorkspaceVersions(cargoToml: string, version: string): string {
  let table = "";
  let foundPackageVersion = false;

  const lines = cargoToml.split(/\r?\n/).map((line) => {
    const trimmed = line.trim();
    if (/^\[+[^\]]+\]+$/.test(trimmed)) {
      table = trimmed;
      return line;
    }

    if (table === "[workspace.package]" && /^\s*version\s*=/.test(line)) {
      foundPackageVersion = true;
      return `version = "${version}"`;
    }

    if (table === "[workspace.dependencies]" && /\bpath\s*=\s*"crates\//.test(line)) {
      return line.replace(/\bversion\s*=\s*"[^"]+"/, `version = "${version}"`);
    }

    return line;
  });

  if (!foundPackageVersion) {
    throw new Error(`Could not find [workspace.package] version in ${cargoTomlPath}`);
  }

  return lines.join("\n");
}
