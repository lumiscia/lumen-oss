import { createRequire } from "node:module";
import process from "node:process";

/** The path of the `lumen` binary installed for this platform, or an error saying how to get one. */
export function resolveBinary(): string {
  const platformPackage = `@lumiscia/lumen-cli-${process.platform}-${process.arch}`;
  try {
    return createRequire(import.meta.url).resolve(`${platformPackage}/bin/lumen`);
  } catch {
    throw new Error(
      `${platformPackage} is not installed. Prebuilt binaries exist for Linux (glibc) on ` +
        "x64 and arm64 and for macOS on arm64, and are optional dependencies, so an install that " +
        "omits optional dependencies leaves them out. On other systems, build from source: " +
        "https://github.com/lumiscia/lumen-oss/tree/main/crates/cli",
    );
  }
}
