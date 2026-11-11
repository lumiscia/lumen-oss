//! Edits to one of the seed models: numbers in its JSON set to values at the edges of what counts,
//! offsets and indices hold, and bytes of its binary chunk overwritten, so that every input is a
//! well-formed `.glb` whose JSON parses.

#![no_main]

mod check;

use std::sync::LazyLock;

use libfuzzer_sys::arbitrary::{self, Arbitrary};
use serde_json::Value;

const SEEDS: [&[u8]; 8] = [
    include_bytes!("../../tests/fixtures/grid.glb"),
    include_bytes!("../../tests/fixtures/grid-draco.glb"),
    include_bytes!("../../tests/fixtures/grid-meshopt.glb"),
    include_bytes!("../../tests/fixtures/grid-etc1s.glb"),
    include_bytes!("../../tests/fixtures/grid-uastc.glb"),
    include_bytes!("../../tests/fixtures/grid-webp.glb"),
    include_bytes!("../seeds/triangle.glb"),
    include_bytes!("../seeds/rich.glb"),
];

/// A seed's JSON, where each number in it is, and its binary chunk.
struct Seed {
    json: Value,
    numbers: Vec<String>,
    bin: Vec<u8>,
}

static PARSED: LazyLock<Vec<Seed>> = LazyLock::new(|| {
    SEEDS
        .iter()
        .map(|bytes| {
            let length = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
            let json: Value = serde_json::from_slice(&bytes[20..20 + length]).unwrap();
            let mut numbers = Vec::new();
            number_paths(&json, String::new(), &mut numbers);
            let bin = bytes.get(28 + length..).unwrap_or_default().to_vec();
            Seed { json, numbers, bin }
        })
        .collect()
});

fn number_paths(value: &Value, path: String, out: &mut Vec<String>) {
    match value {
        Value::Number(_) => out.push(path),
        Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                number_paths(item, format!("{path}/{index}"), out);
            }
        }
        Value::Object(fields) => {
            for (key, item) in fields {
                number_paths(item, format!("{path}/{key}"), out);
            }
        }
        _ => {}
    }
}

#[derive(Arbitrary, Debug)]
enum Number {
    Boundary(u8),
    Small(i16),
    Large(u64),
    Float(f64),
    /// The number at another place in the same document.
    Copied(u16),
}

const BOUNDARIES: [u64; 12] = [
    0,
    1,
    2,
    3,
    4,
    255,
    256,
    65_535,
    65_536,
    u32::MAX as u64,
    1 << 32,
    (1 << 53) + 1,
];

#[derive(Arbitrary, Debug)]
enum Edit {
    Number { at: u16, to: Number },
    Byte { at: u32, to: u8 },
    Bytes { at: u32, to: Vec<u8> },
}

#[derive(Arbitrary, Debug)]
struct Input {
    seed: u8,
    edits: Vec<Edit>,
}

libfuzzer_sys::fuzz_target!(|input: Input| {
    let seed = &PARSED[usize::from(input.seed) % PARSED.len()];
    let (mut json, mut bin) = (seed.json.clone(), seed.bin.clone());
    for edit in input.edits.into_iter().take(16) {
        match edit {
            Edit::Number { at, to } => {
                let path = &seed.numbers[usize::from(at) % seed.numbers.len()];
                let value = match to {
                    Number::Boundary(index) => {
                        Value::from(BOUNDARIES[usize::from(index) % BOUNDARIES.len()])
                    }
                    Number::Small(value) => Value::from(value),
                    Number::Large(value) => Value::from(value),
                    Number::Float(value) => Value::from(value),
                    Number::Copied(from) => {
                        let from = &seed.numbers[usize::from(from) % seed.numbers.len()];
                        seed.json.pointer(from).unwrap().clone()
                    }
                };
                *json.pointer_mut(path).unwrap() = value;
            }
            Edit::Byte { at, to } if !bin.is_empty() => {
                let length = bin.len();
                bin[at as usize % length] = to;
            }
            Edit::Bytes { at, to } if !bin.is_empty() => {
                let start = at as usize % bin.len();
                let end = (start + to.len()).min(bin.len());
                bin[start..end].copy_from_slice(&to[..end - start]);
            }
            _ => {}
        }
    }
    check::check(&glb(&json, &bin));
});

/// `json` and `bin` as a `.glb`.
fn glb(json: &Value, bin: &[u8]) -> Vec<u8> {
    let mut json = serde_json::to_vec(json).unwrap();
    json.resize(json.len().next_multiple_of(4), b' ');
    let mut bin = bin.to_vec();
    bin.resize(bin.len().next_multiple_of(4), 0);
    let mut out = b"glTF".to_vec();
    out.extend(2u32.to_le_bytes());
    out.extend(((12 + 8 + json.len() + 8 + bin.len()) as u32).to_le_bytes());
    out.extend((json.len() as u32).to_le_bytes());
    out.extend(b"JSON");
    out.extend(json);
    out.extend((bin.len() as u32).to_le_bytes());
    out.extend(b"BIN\0");
    out.extend(bin);
    out
}
