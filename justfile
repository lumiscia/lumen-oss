initial_memory := "134217728"
max_memory := "2147483648"
wasm_out := justfile_directory() / "packages/lumen-wasm/src/internal"
wasm_crate := "lumen-wasm"
wasm_name := "lumen_wasm"
wasm_target := "wasm32-unknown-unknown"
wasm_debug := justfile_directory() / "target" / wasm_target / "debug" / wasm_name + ".wasm"
wasm_release := justfile_directory() / "target" / wasm_target / "release" / wasm_name + ".wasm"
node_specs_out := justfile_directory() / "packages/lumen-node-specs"
definitions_out := justfile_directory() / "generated/lumen-definitions"
generators_config := justfile_directory() / "crates/lumen-generators/package.config.json"

debug:
    RUSTFLAGS="-C link-arg=--initial-memory={{ initial_memory }} -C link-arg=--max-memory={{ max_memory }}" \
        cargo build --package {{ wasm_crate }} --target {{ wasm_target }}
    rm -rf {{ wasm_out }}
    wasm-bindgen --target bundler --debug --keep-debug --out-dir {{ wasm_out }} --out-name {{ wasm_name }} {{ wasm_debug }}

release:
    RUSTFLAGS="-C link-arg=--initial-memory={{ initial_memory }} -C link-arg=--max-memory={{ max_memory }}" \
        cargo build --release --package {{ wasm_crate }} --target {{ wasm_target }}
    rm -rf {{ wasm_out }}
    wasm-bindgen --target bundler --out-dir {{ wasm_out }} --out-name {{ wasm_name }} {{ wasm_release }}
    vp exec wasm-opt -Oz -o {{ wasm_out / wasm_name + "_bg.wasm" }} {{ wasm_out / wasm_name + "_bg.wasm" }}

clean-node-specs:
    rm -rf {{ node_specs_out }}

generate-node-specs: clean-node-specs
    cargo run -p lumen-generators -- meta-package --config {{ generators_config }} --out-dir {{ node_specs_out }}

clean-definitions:
    rm -rf {{ definitions_out }}

generate-definitions: clean-definitions
    cargo run -p lumen-generators -- definitions --out-dir {{ definitions_out }}
