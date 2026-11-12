definitions_out := justfile_directory() / "definitions"
wasm_bindings_out := justfile_directory() / "packages/lumen-bindings/wasm"
generated_types_out := justfile_directory() / "packages/lumen-types/src/generated"
node_reference := justfile_directory() / "docs/node-reference.md"

# The TypeScript half of CI's Check job. CI also runs Rust formatting, checks and tests, and the
# lavapipe-backed SDK integration test.
ready: verify-typescript-artifacts
    pnpm build
    pnpm check
    pnpm test

wasm-bindings-debug:
    pnpm --filter @lumiscia/generate-bindings generate -- debug --out-dir {{ wasm_bindings_out }}

wasm-bindings-release:
    pnpm --filter @lumiscia/generate-bindings generate -- release --out-dir {{ wasm_bindings_out }}

# Also writes docs/node-reference.md from the definitions.
generate-types: generate-definitions
    pnpm --filter @lumiscia/generate-types generate

# Rewrites the JSON blocks of docs/3d.md from the TypeScript blocks after them.
update-3d-guide:
    UPDATE_GUIDE=1 pnpm exec vitest run packages/lumen-sdk/test/guide.test.ts
    pnpm exec oxfmt --write docs/3d.md

# Sets the Lumen major.minor compositions declare as `lumenSchemaVersion`: the engine constant
# and every tracked fixture, example and doc, then the generated output. Run it with a release
# line's new major.minor after its version bump.
set-schema-version version:
    pnpm --filter @lumiscia/release-tools set-schema-version {{version}}
    just generate-types

# Also covers verify-definitions, since the generated schema is among the checked paths.
verify-typescript-artifacts: generate-types wasm-bindings-debug
    #!/usr/bin/env bash
    set -euo pipefail
    paths=({{ definitions_out }}/composition.schema.json {{ node_reference }} {{ generated_types_out }})
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
    cargo run -p lumen-generators -- {{ definitions_out }}
    pnpm exec oxfmt --write {{ definitions_out }}

verify-definitions:
    #!/usr/bin/env bash
    set -euo pipefail
    tmpdir="$(mktemp -d)"
    trap 'rm -rf "$tmpdir"' EXIT
    cargo run -p lumen-generators -- "$tmpdir"
    pnpm exec oxfmt --write "$tmpdir"
    diff -u {{ definitions_out }}/composition.schema.json "$tmpdir/composition.schema.json"

# Checks every Rust dependency's license against deny.toml, as CI does. Needs cargo-deny.
licenses:
    cargo deny check licenses
