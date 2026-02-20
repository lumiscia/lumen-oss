fn main() {
	let target = std::env::var("TARGET").unwrap_or_default();
	if target == "wasm32-unknown-emscripten" {
		println!("cargo:rustc-link-arg=--no-entry");
	}
}
