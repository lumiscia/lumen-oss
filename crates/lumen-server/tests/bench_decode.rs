#![cfg(feature = "legacy-decode-tests")]

//! Legacy decode benchmark placeholder.
//!
//! The previous benchmark targeted the removed pre-composition APIs
//! (`lumen::compile`, `lumen::model`, `lumen_server::video`).
//! Re-port this benchmark once the decode backend benchmarks are rebuilt
//! on top of the current `lumen::Composition` render path.

#[test]
fn legacy_bench_decode_pending_port() {
    eprintln!("bench_decode is pending port to the current lumen API");
}
