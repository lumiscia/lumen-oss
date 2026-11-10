//! `KHR_draco_mesh_compression`: a triangle primitive's attributes and indices decoded from its
//! Draco stream by the `draco-core` crate.
//!
//! The counts the primitive's accessors declare are checked against the limits and charged to the
//! compressed geometry budget before decoding, and bound what the decoder may produce: as many
//! faces as the indices declare, and attribute values of [`DECODED_BYTES_PER_POINT`] for each
//! vertex declared across every attribute in the stream, read or not. A mesh is decoded on the
//! calling thread. What it decodes to must then match what was declared: its points the vertex
//! count, its faces the index count, and each attribute read its accessor's components and type.

use draco_core::{DataType, DecodeLimits, DecoderBuffer, Mesh, MeshDecoder};
use gltf_json::accessor::ComponentType;
use gltf_json::validation::Checked;
use gltf_json::{Accessor, Index};
use serde_json::{Map, Value};

use super::buffers::Buffers;
use super::{ModelError, guarded, invalid};

const EXTENSION: &str = "KHR_draco_mesh_compression";
/// The most bytes of attribute values a Draco stream may decode to for each vertex declared,
/// across all of its attributes: room for eight of four floats each.
const DECODED_BYTES_PER_POINT: u64 = 128;

/// A primitive's Draco extension, when it has one.
pub(super) fn extension(primitive: &gltf_json::mesh::Primitive) -> Option<&Value> {
    primitive
        .extensions
        .as_ref()
        .and_then(|extensions| extensions.others.get(EXTENSION))
}

/// A decoded Draco mesh and the attributes of the stream the extension names, by semantic.
pub(super) struct Decoded<'a> {
    mesh: Mesh,
    attributes: &'a Map<String, Value>,
}

fn malformed(why: &str) -> ModelError {
    invalid(format!("a Draco-compressed primitive {why}"))
}

/// Decodes the stream of `extension`, a primitive's Draco extension, whose accessors declare
/// `count` vertices and `index_count` indices. Each `(semantic, components)` of `read` the stream
/// holds is charged at four bytes a component a vertex, and the indices at four bytes each.
pub(super) fn decode<'a>(
    buffers: &Buffers<'_>,
    extension: &'a Value,
    count: usize,
    index_count: usize,
    read: &[(&str, usize)],
) -> Result<Decoded<'a>, ModelError> {
    let view = extension
        .get("bufferView")
        .and_then(Value::as_u64)
        .and_then(|view| u32::try_from(view).ok())
        .ok_or_else(|| malformed("names no buffer view"))?;
    let attributes = extension
        .get("attributes")
        .and_then(Value::as_object)
        .ok_or_else(|| malformed("has no attributes"))?;
    let components: usize = read
        .iter()
        .filter(|(semantic, _)| attributes.contains_key(*semantic))
        .map(|(_, components)| components)
        .sum();
    let charge = count
        .checked_mul(components)
        .and_then(|values| values.checked_add(index_count))
        .and_then(|values| values.checked_mul(4))
        .unwrap_or(usize::MAX);
    buffers.spend_compressed(charge)?;
    let (bytes, _) = buffers.view(Index::new(view))?;

    let limits = DecodeLimits::default()
        .with_max_points(count as u64)
        .with_max_faces((index_count / 3) as u64)
        .with_max_decoded_bytes((count as u64).saturating_mul(DECODED_BYTES_PER_POINT));
    let mesh = guarded("a Draco-compressed primitive", || {
        let mut buffer = DecoderBuffer::new(bytes).with_limits(limits);
        let mut mesh = Mesh::new();
        MeshDecoder::new()
            .decode(&mut buffer, &mut mesh)
            .map_err(|error| {
                invalid(format!(
                    "a Draco-compressed primitive in the model is corrupt: {error}"
                ))
            })?;
        Ok(mesh)
    })?;
    if mesh.num_points() != count || Some(mesh.num_faces()) != index_count.checked_div(3) {
        return Err(malformed(
            "decodes to a different number of vertices or indices than its accessors declare",
        ));
    }
    Ok(Decoded { mesh, attributes })
}

impl Decoded<'_> {
    /// The values of the attribute under `semantic`, `N` an element, as floats normalized as
    /// [`Buffers::floats`] normalizes them, or `None` when the stream doesn't hold it.
    pub(super) fn vectors<const N: usize>(
        &self,
        semantic: &str,
        accessor: &Accessor,
        normalized: Option<bool>,
    ) -> Result<Option<Vec<[f32; N]>>, ModelError> {
        let Some(id) = self.attributes.get(semantic) else {
            return Ok(None);
        };
        let attribute = id
            .as_u64()
            .and_then(|id| u32::try_from(id).ok())
            .and_then(|id| self.mesh.attribute_by_unique_id(id))
            .ok_or_else(|| malformed(&format!("lacks the attribute for {semantic}")))?;
        let (Checked::Valid(component), Checked::Valid(kind)) =
            (&accessor.component_type, &accessor.type_)
        else {
            return Err(invalid("an accessor has an unknown component type or type"));
        };
        let max = match (component.0, attribute.data_type()) {
            (ComponentType::F32, DataType::Float32) | (ComponentType::U32, DataType::Uint32) => {
                None
            }
            (ComponentType::U8, DataType::Uint8) => Some(255.0),
            (ComponentType::I8, DataType::Int8) => Some(127.0),
            (ComponentType::U16, DataType::Uint16) => Some(65535.0),
            (ComponentType::I16, DataType::Int16) => Some(32767.0),
            _ => {
                return Err(malformed(&format!(
                    "holds {semantic} in another type than its accessor declares"
                )));
            }
        };
        if usize::from(attribute.num_components()) != N || kind.multiplicity() != N {
            return Err(malformed(&format!(
                "holds {semantic} with another number of components than its accessor, or \
                 than {N}"
            )));
        }
        let mut values = attribute.read_f32s(self.mesh.num_points(), N);
        if let Some(max) = max.filter(|_| normalized.unwrap_or(accessor.normalized)) {
            for value in &mut values {
                *value = (*value / max).max(-1.0);
            }
        }
        Ok(Some(values.as_chunks::<N>().0.to_vec()))
    }

    /// The mesh's triangles' indices, three to a face.
    pub(super) fn indices(&self) -> Vec<u32> {
        self.mesh
            .faces()
            .iter()
            .flat_map(|face| face.map(|point| point.0))
            .collect()
    }
}
