//! The binary glTF container: a header, a JSON chunk and an optional binary chunk.

use super::{
    MAX_JSON_BYTES, MAX_JSON_ITEMS, MAX_MODEL_BYTES, ModelError, invalid, too_large, unsupported,
};

pub(super) struct Glb<'a> {
    pub(super) json: &'a [u8],
    pub(super) bin: Option<&'a [u8]>,
}

const JSON: [u8; 4] = *b"JSON";
const BIN: [u8; 4] = *b"BIN\0";

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    let word = bytes.get(at..at.checked_add(4)?)?;
    Some(u32::from_le_bytes(word.try_into().ok()?))
}

/// The chunks of `bytes`, which must be a glTF 2.0 binary file. Chunks of other types after the
/// JSON are skipped, as the format says.
pub(super) fn split(bytes: &[u8]) -> Result<Glb<'_>, ModelError> {
    if bytes.len() > MAX_MODEL_BYTES {
        return Err(too_large("bytes", MAX_MODEL_BYTES));
    }
    if !bytes.starts_with(b"glTF") {
        return Err(invalid(
            "not a binary glTF (.glb) file: it doesn't start with `glTF`",
        ));
    }
    let (Some(version), Some(length)) = (u32_at(bytes, 4), u32_at(bytes, 8)) else {
        return Err(invalid("the file ends inside its glTF header"));
    };
    if version != 2 {
        return Err(unsupported(format!(
            "the file is glTF version {version}; only glTF 2.0 is read"
        )));
    }
    let length = usize::try_from(length).unwrap_or(usize::MAX);
    let mut rest = bytes
        .get(12..length)
        .ok_or_else(|| invalid("the file is shorter than its glTF header says"))?;
    let mut json = None;
    let mut bin = None;
    while !rest.is_empty() {
        let (Some(chunk_length), Some(kind)) = (u32_at(rest, 0), rest.get(4..8)) else {
            return Err(invalid("the file ends inside a chunk header"));
        };
        let end = usize::try_from(chunk_length)
            .ok()
            .and_then(|length| length.checked_add(8));
        let data = end
            .and_then(|end| rest.get(8..end))
            .ok_or_else(|| invalid("a chunk reaches past the end of the file"))?;
        match (json.is_none(), kind) {
            (true, kind) if kind == JSON => json = Some(data),
            (true, _) => return Err(invalid("the file's first chunk isn't JSON")),
            (false, kind) if kind == BIN && bin.is_none() => bin = Some(data),
            _ => {}
        }
        rest = &rest[8 + data.len()..];
    }
    let json = json.ok_or_else(|| invalid("the file has no JSON chunk"))?;
    check_json_size(json)?;
    Ok(Glb { json, bin })
}

/// Rejects JSON that would allocate too much once parsed, before parsing it: what it allocates
/// grows with its objects and strings, which a small file can hold millions of.
fn check_json_size(json: &[u8]) -> Result<(), ModelError> {
    if json.len() > MAX_JSON_BYTES {
        return Err(too_large("bytes of JSON", MAX_JSON_BYTES));
    }
    let mut items = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for &byte in json {
        if in_string {
            match (escaped, byte) {
                (true, _) => escaped = false,
                (false, b'\\') => escaped = true,
                (false, b'"') => in_string = false,
                _ => {}
            }
            continue;
        }
        if byte == b'"' {
            in_string = true;
        } else if byte != b'{' {
            continue;
        }
        items += 1;
        if items > MAX_JSON_ITEMS {
            return Err(too_large("JSON objects and strings", MAX_JSON_ITEMS));
        }
    }
    Ok(())
}
