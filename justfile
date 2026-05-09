definitions_out := justfile_directory() / "definitions"
wasm_bindings_out := justfile_directory() / "packages/lumen-bindings/src"
release_artifacts_out := justfile_directory() / "generated/release"

ready: ci-typescript-artifacts verify-definitions
    pnpm check
    pnpm test
    pnpm build

wasm-bindings-debug:
    bun crates/lumen-wasm/tooling/generate-bindings.ts debug --out-dir {{ wasm_bindings_out }}

wasm-bindings-release:
    bun crates/lumen-wasm/tooling/generate-bindings.ts release --out-dir {{ wasm_bindings_out }}

generate-types: generate-definitions
    pnpm --filter @lumiscia/generate-types generate

ci-typescript-artifacts: generate-types wasm-bindings-debug

clean-release-artifacts:
    rm -rf {{ release_artifacts_out }}

release-artifacts: clean-release-artifacts wasm-bindings-release generate-definitions
    mkdir -p {{ release_artifacts_out }}
    cp -R {{ wasm_bindings_out }}/* {{ release_artifacts_out }}/
    cp -R {{ definitions_out }} {{ release_artifacts_out }}/definitions
    find {{ release_artifacts_out }} -type f -print0 | sort -z | xargs -0 shasum -a 256 > {{ release_artifacts_out }}/checksums.txt

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
    diff -u {{ definitions_out }}/meta.json "$tmpdir/meta.json"
    diff -u {{ definitions_out }}/schemas/meta.schema.json "$tmpdir/schemas/meta.schema.json"
    diff -u {{ definitions_out }}/schemas/composition.schema.json "$tmpdir/schemas/composition.schema.json"
