//! The binary glTF container: a header, a JSON chunk and an optional binary chunk.

use super::{MAX_MODEL_BYTES, ModelError, invalid, too_large, unsupported};

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

/// The chunks of `bytes`, which must be a glTF 2.0 binary file: one JSON chunk first, at most one
/// binary chunk, and chunks of other types after the JSON, which are skipped as the format says.
/// Every chunk's length is a multiple of four. Bytes past the length the header declares are
/// ignored.
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
        if !chunk_length.is_multiple_of(4) {
            return Err(invalid("a chunk's length isn't a multiple of four bytes"));
        }
        let end = usize::try_from(chunk_length)
            .ok()
            .and_then(|length| length.checked_add(8));
        let data = end
            .and_then(|end| rest.get(8..end))
            .ok_or_else(|| invalid("a chunk reaches past the end of the file"))?;
        match (json.is_none(), kind) {
            (true, kind) if kind == JSON => json = Some(data),
            (true, _) => return Err(invalid("the file's first chunk isn't JSON")),
            (false, kind) if kind == JSON => return Err(invalid("the file has two JSON chunks")),
            (false, kind) if kind == BIN && bin.is_some() => {
                return Err(invalid("the file has two binary chunks"));
            }
            (false, kind) if kind == BIN => bin = Some(data),
            _ => {}
        }
        rest = &rest[8 + data.len()..];
    }
    let json = json.ok_or_else(|| invalid("the file has no JSON chunk"))?;
    Ok(Glb { json, bin })
}
