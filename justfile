node_specs_out := justfile_directory() / "packages/lumen-node-specs"
definitions_out := justfile_directory() / "generated/lumen-definitions"
wasm_bindings_out := justfile_directory() / "generated/lumen-bindings/src"
generators_config := justfile_directory() / "crates/lumen-generators/package.config.json"

debug:
    bun crates/lumen-wasm/tooling/generate-bindings.ts debug --out-dir {{ wasm_bindings_out }}

release:
    bun crates/lumen-wasm/tooling/generate-bindings.ts release --out-dir {{ wasm_bindings_out }}

clean-node-specs:
    rm -rf {{ node_specs_out }}

generate-node-specs: clean-node-specs
    cargo run -p lumen-generators -- meta-package --config {{ generators_config }} --out-dir {{ node_specs_out }}

clean-definitions:
    rm -rf {{ definitions_out }}

generate-definitions: clean-definitions
    cargo run -p lumen-generators -- definitions --out-dir {{ definitions_out }}
