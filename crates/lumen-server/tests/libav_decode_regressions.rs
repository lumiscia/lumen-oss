#![cfg(feature = "legacy-decode-tests")]

//! Legacy libav decode regression placeholder.
//!
//! The original regression tests exercised the removed pre-composition render backend
//! (`lumen_server::video`) and old `lumen` project model APIs.
//! Reintroduce equivalent coverage using the current composition JSON + render pipeline.

#[test]
fn legacy_libav_decode_regressions_pending_port() {
    eprintln!("libav decode regressions are pending port to the current lumen API");
}
