node_specs_out := justfile_directory() / "packages/lumen-node-specs"
definitions_out := justfile_directory() / "generated/lumen-definitions"
wasm_bindings_out := justfile_directory() / "packages/lumen-bindings/src"
release_artifacts_out := justfile_directory() / "generated/release"
generators_config := justfile_directory() / "crates/lumen-generators/package.config.json"

debug:
    bun crates/lumen-wasm/tooling/generate-bindings.ts debug --out-dir {{ wasm_bindings_out }}

release:
    bun crates/lumen-wasm/tooling/generate-bindings.ts release --out-dir {{ wasm_bindings_out }}

clean-release-artifacts:
    rm -rf {{ release_artifacts_out }}

release-artifacts: clean-release-artifacts release generate-definitions
    mkdir -p {{ release_artifacts_out }}
    cp -R {{ wasm_bindings_out }}/* {{ release_artifacts_out }}/
    cp -R {{ definitions_out }} {{ release_artifacts_out }}/definitions
    find {{ release_artifacts_out }} -type f -print0 | sort -z | xargs -0 shasum -a 256 > {{ release_artifacts_out }}/checksums.txt

clean-node-specs:
    rm -rf {{ node_specs_out }}

generate-node-specs: clean-node-specs
    cargo run -p lumen-generators -- meta-package --config {{ generators_config }} --out-dir {{ node_specs_out }}

clean-definitions:
    rm -rf {{ definitions_out }}

generate-definitions: clean-definitions
    cargo run -p lumen-generators -- definitions --out-dir {{ definitions_out }}
