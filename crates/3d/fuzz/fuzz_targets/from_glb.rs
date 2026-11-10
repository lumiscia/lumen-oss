//! Any bytes, as a `.glb`.

#![no_main]

mod check;

libfuzzer_sys::fuzz_target!(|bytes: &[u8]| check::check(bytes));
