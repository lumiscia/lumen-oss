initial_memory := "134217728"
max_memory := "2147483648"

wasm_out := justfile_directory() / "packages/lumen-wasm"

debug:
    RUSTFLAGS="-C link-arg=--initial-memory={{initial_memory}} -C link-arg=--max-memory={{max_memory}}" \
        wasm-pack build --dev --target bundler --out-dir {{wasm_out}} crates/lumen-wasm

release:
    RUSTFLAGS="-C link-arg=--initial-memory={{initial_memory}} -C link-arg=--max-memory={{max_memory}}" \
        wasm-pack build --release --target bundler --out-dir {{wasm_out}} crates/lumen-wasm
