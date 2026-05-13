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
const updated = cargoToml.replace(
  /(\[workspace\.package\][\s\S]*?\nversion = )"[^"]+"/,
  `$1"${version}"`,
);

if (updated === cargoToml) {
  throw new Error(`Could not find [workspace.package] version in ${cargoTomlPath}`);
}

await writeFile(cargoTomlPath, updated);
console.log(`Synced Cargo workspace version to ${version}`);
