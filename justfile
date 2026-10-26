definitions_out := justfile_directory() / "definitions"
wasm_bindings_out := justfile_directory() / "packages/lumen-bindings/src"
generated_types_out := justfile_directory() / "packages/lumen-types/src/generated"

ready: verify-typescript-artifacts verify-definitions
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

verify-typescript-artifacts: ci-typescript-artifacts
    #!/usr/bin/env bash
    set -euo pipefail
    paths=({{ definitions_out }}/composition.schema.json {{ generated_types_out }} {{ wasm_bindings_out }})
    # `generated/` is gitignored, so new type files only show up with --ignored.
    changes="$(git status --porcelain -- "${paths[@]}")$(git status --porcelain --ignored -- {{ generated_types_out }})"
    if [ -n "$changes" ]; then
        echo "$changes"
        git diff -- "${paths[@]}"
        exit 1
    fi

# The round-trip fixture `crates/engine/tests/ts_expressions.rs` checks; `pnpm test` fails when it is stale.
generate-expression-fixture:
    pnpm --filter @lumiscia/lumen-types build
    pnpm exec vitest run packages/lumen-shared/test/expression.test.ts --update

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
