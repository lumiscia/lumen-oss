# Windows Development

Windows is a basic development target for the TypeScript workspace and pure Rust crates that do not depend on local FFmpeg or native renderer system libraries.

## Current CI Coverage

The Windows CI job runs on `windows-latest` and checks:

- TypeScript package builds, excluding generated WASM bindings.
- Pure Rust crates: `lumen-engine`, `lumen-engine-gpu`, `lumen-generators`, `lumen-engine-macros`, and `lumen-engine-text`.

This coverage is intended to catch path separator issues, shell portability problems in package scripts, and cross-platform compile regressions in the core schema/model crates.

## Local Setup

Install:

- Node.js 22 or newer.
- pnpm 10.33.0.
- Rust stable with Cargo.
- `just`.

Then run:

```powershell
pnpm install
pnpm -r --filter './packages/*' --filter '!@lumiscia/lumen-bindings' build
cargo check -p lumen-engine -p lumen-engine-gpu -p lumen-generators -p lumen-engine-macros -p lumen-engine-text
```

## Known Gaps

The following Windows paths are not supported yet:

- `lumen-server` build and runtime behavior.
- FFmpeg-backed decode and encode crates.
- Native Vulkan rendering validation on Windows adapters.
- WASM/type generation scripts that rely on Unix shell snippets.
- Hardware encoding paths.

When these paths are added, the Windows CI job should expand to cover them directly instead of relying on local validation.
