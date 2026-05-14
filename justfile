definitions_out := justfile_directory() / "definitions"
wasm_bindings_out := justfile_directory() / "packages/lumen-bindings/src"

ready: ci-typescript-artifacts verify-definitions
    pnpm check
    pnpm test
    pnpm build

wasm-bindings-debug:
    pnpm --filter @lumiscia/generate-bindings generate -- debug --out-dir {{ wasm_bindings_out }}

wasm-bindings-release:
    pnpm --filter @lumiscia/generate-bindings generate -- release --out-dir {{ wasm_bindings_out }}

generate-types: generate-definitions
    pnpm --filter @lumiscia/generate-types generate

ci-typescript-artifacts: generate-types wasm-bindings-debug

generate-definitions:
    cargo run -p lumen-generators -- definitions --out-dir {{ definitions_out }}
    pnpm exec oxfmt --write {{ definitions_out }}

verify-definitions:
    #!/usr/bin/env bash
    set -euo pipefail
    tmpdir="$(mktemp -d)"
    trap 'rm -rf "$tmpdir"' EXIT
    cargo run -p lumen-generators -- definitions --out-dir "$tmpdir"
    pnpm exec oxfmt --write "$tmpdir"
    diff -u {{ definitions_out }}/composition.schema.json "$tmpdir/composition.schema.json"
