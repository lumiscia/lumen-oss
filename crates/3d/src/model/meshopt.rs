//! `EXT_meshopt_compression`: buffer views decoded by the meshopt codec and its filters, with the
//! layout each declares checked, and its decoded size charged to the budget, before decoding.
//!
//! The codec reads at most the compressed bytes it is given and writes exactly the decoded size
//! declared, so decoding takes time and memory in proportion to that size.

use std::collections::HashMap;

use gltf_json::Root;
use serde_json::Value;

use super::buffers::{Buffers, size};
use super::{MAX_VIEW_BYTES, ModelError, guarded, invalid, too_large};

const EXTENSION: &str = "EXT_meshopt_compression";

/// Whether `buffer` is a fallback buffer, which holds no data of its own when the extension is
/// required, so that only compressed views may name it.
pub(super) fn is_fallback(buffer: &gltf_json::Buffer) -> bool {
    buffer
        .extensions
        .as_ref()
        .and_then(|extensions| extensions.others.get(EXTENSION))
        .and_then(|extension| extension.get("fallback"))
        .and_then(Value::as_bool)
        == Some(true)
}

/// The decoded bytes of each of `root`'s compressed buffer views, by its index.
pub(super) fn decode_views(
    root: &Root,
    buffers: &Buffers<'_>,
) -> Result<HashMap<usize, Vec<u8>>, ModelError> {
    let mut views = HashMap::new();
    for (index, view) in root.buffer_views.iter().enumerate() {
        let Some(extension) = view
            .extensions
            .as_ref()
            .and_then(|extensions| extensions.others.get(EXTENSION))
        else {
            continue;
        };
        let declared = size(view.byte_length.0, "a buffer view's length")?;
        views.insert(index, decode_view(buffers, extension, declared)?);
    }
    Ok(views)
}

fn malformed(why: &str) -> ModelError {
    invalid(format!("a meshopt-compressed buffer view {why}"))
}

/// The unsigned integer under `key` of `extension`.
fn number(extension: &Value, key: &str) -> Result<Option<usize>, ModelError> {
    match extension.get(key) {
        None => Ok(None),
        Some(value) => value
            .as_u64()
            .and_then(|value| usize::try_from(value).ok())
            .map(Some)
            .ok_or_else(|| malformed(&format!("has a `{key}` that isn't a size"))),
    }
}

/// The view `extension` describes, decoded to `declared` bytes, the length of the view it
/// stands for.
fn decode_view(
    buffers: &Buffers<'_>,
    extension: &Value,
    declared: usize,
) -> Result<Vec<u8>, ModelError> {
    let required =
        |key: &str| number(extension, key)?.ok_or_else(|| malformed(&format!("has no `{key}`")));
    let (buffer, length) = (required("buffer")?, required("byteLength")?);
    let (stride, count) = (required("byteStride")?, required("count")?);
    let offset = number(extension, "byteOffset")?.unwrap_or(0);
    let mode = extension.get("mode").and_then(Value::as_str);
    let filter = match extension.get("filter") {
        None => "NONE",
        Some(filter) => filter
            .as_str()
            .ok_or_else(|| malformed("has a `filter` that isn't a name"))?,
    };
    let source = buffers
        .buffer(buffer)?
        .get(offset..offset.saturating_add(length))
        .ok_or_else(|| malformed("reaches past the end of its buffer"))?;
    let decoded = count
        .checked_mul(stride)
        .filter(|&decoded| decoded <= MAX_VIEW_BYTES)
        .ok_or_else(|| too_large("bytes in one buffer view", MAX_VIEW_BYTES))?;
    if decoded != declared {
        return Err(malformed(
            "decodes to a different length than its buffer view has",
        ));
    }
    match (mode, stride, filter) {
        (Some("ATTRIBUTES"), 4..=256, _) if stride.is_multiple_of(4) => {}
        (Some("ATTRIBUTES"), ..) => {
            return Err(malformed(
                "of attributes has a stride that isn't a multiple of four up to 256",
            ));
        }
        (Some("TRIANGLES" | "INDICES"), 2 | 4, "NONE") => {}
        (Some("TRIANGLES" | "INDICES"), ..) => {
            return Err(malformed(
                "of indices has a stride other than two or four bytes, or a filter",
            ));
        }
        (Some(mode), ..) => return Err(malformed(&format!("has an unknown mode `{mode}`"))),
        (None, ..) => return Err(malformed("has no `mode`")),
    }
    if mode == Some("TRIANGLES") && !count.is_multiple_of(3) {
        return Err(malformed(
            "of triangles has a count that isn't a multiple of three",
        ));
    }
    match (filter, stride) {
        ("NONE" | "EXPONENTIAL", _) | ("OCTAHEDRAL", 4 | 8) | ("QUATERNION", 8) => {}
        ("OCTAHEDRAL" | "QUATERNION", _) => {
            return Err(malformed("has a stride its filter can't take"));
        }
        (filter, _) => return Err(malformed(&format!("has an unknown filter `{filter}`"))),
    }
    buffers.spend_meshopt(decoded)?;

    let corrupt = |error: meshoptimizer::Error| {
        invalid(format!(
            "a meshopt-compressed buffer view in the model is corrupt: {error}"
        ))
    };
    guarded("a meshopt-compressed buffer view", || {
        let length = source.len();
        if mode == Some("ATTRIBUTES") {
            let mut out = vec![0u8; decoded];
            meshoptimizer::decode_vertex_buffer(&mut out, count, stride, source, length)
                .map_err(corrupt)?;
            match filter {
                "OCTAHEDRAL" => meshoptimizer::decode_filter_oct(&mut out, count, stride),
                "QUATERNION" => meshoptimizer::decode_filter_quat(&mut out, count, stride),
                "EXPONENTIAL" => meshoptimizer::decode_filter_exp(&mut out, count, stride),
                _ => Ok(()),
            }
            .map_err(corrupt)?;
            return Ok(out);
        }
        let triangles = mode == Some("TRIANGLES");
        if stride == 2 {
            let mut indices = vec![0u16; count];
            if triangles {
                meshoptimizer::decode_index_buffer(&mut indices, count, source, length)
            } else {
                meshoptimizer::decode_index_sequence(&mut indices, count, source, length)
            }
            .map_err(corrupt)?;
            Ok(indices
                .iter()
                .flat_map(|index| index.to_le_bytes())
                .collect())
        } else {
            let mut indices = vec![0u32; count];
            if triangles {
                meshoptimizer::decode_index_buffer(&mut indices, count, source, length)
            } else {
                meshoptimizer::decode_index_sequence(&mut indices, count, source, length)
            }
            .map_err(corrupt)?;
            Ok(indices
                .iter()
                .flat_map(|index| index.to_le_bytes())
                .collect())
        }
    })
}
