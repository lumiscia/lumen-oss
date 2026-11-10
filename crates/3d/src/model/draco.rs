//! `KHR_draco_mesh_compression`: a triangle primitive's attributes and indices decoded from its
//! Draco stream by the `draco-core` crate.
//!
//! Before decoding, the counts the primitive's accessors declare are checked against the limits,
//! and the decoder's whole allowance is charged to the Draco budget: as many faces as the indices
//! declare, and attribute values of up to [`DECODED_BYTES_PER_POINT`] for each vertex declared,
//! across every attribute in the stream, within what is left of the budget. A mesh is decoded on
//! the calling thread. What it decodes to must then match what was declared: its points the
//! vertex count, its faces the index count, its attributes those the extension names, and each
//! attribute read its accessor's components and type.

use std::collections::HashSet;

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
const DECODED_BYTES_PER_POINT: usize = 128;

/// A primitive's Draco extension, when it has one.
pub(super) fn extension(primitive: &gltf_json::mesh::Primitive) -> Option<&Value> {
    primitive
        .extensions
        .as_ref()
        .and_then(|extensions| extensions.others.get(EXTENSION))
}

fn malformed(why: &str) -> ModelError {
    invalid(format!("a Draco-compressed primitive {why}"))
}

/// A primitive's Draco stream, not yet decoded: its buffer view and the attributes the extension
/// names, by semantic.
pub(super) struct Stream<'a> {
    view: u32,
    attributes: &'a Map<String, Value>,
}

/// A decoded Draco mesh and the attributes of the stream the extension names, by semantic.
pub(super) struct Decoded<'a> {
    mesh: Mesh,
    attributes: &'a Map<String, Value>,
}

impl<'a> Stream<'a> {
    /// The stream `extension`, a primitive's Draco extension, describes.
    pub(super) fn new(extension: &'a Value) -> Result<Self, ModelError> {
        let view = extension
            .get("bufferView")
            .and_then(Value::as_u64)
            .and_then(|view| u32::try_from(view).ok())
            .ok_or_else(|| malformed("names no buffer view"))?;
        let attributes = extension
            .get("attributes")
            .and_then(Value::as_object)
            .ok_or_else(|| malformed("has no attributes"))?;
        Ok(Self { view, attributes })
    }

    /// Whether the stream holds the attribute under `semantic`.
    pub(super) fn holds(&self, semantic: &str) -> bool {
        self.attributes.contains_key(semantic)
    }

    /// Decodes the stream, whose primitive's accessors declare `count` vertices and
    /// `index_count` indices, and of whose attributes each `(semantic, components)` of `read` is
    /// read, at four bytes a component.
    pub(super) fn decode(
        self,
        buffers: &Buffers<'_>,
        count: usize,
        index_count: usize,
        read: &[(&str, usize)],
    ) -> Result<Decoded<'a>, ModelError> {
        let components: usize = read
            .iter()
            .filter(|(semantic, _)| self.holds(semantic))
            .map(|(_, components)| components)
            .sum();
        let needed = count.saturating_mul(components).saturating_mul(4);
        let indices = index_count.saturating_mul(4);
        let allowance = count
            .saturating_mul(DECODED_BYTES_PER_POINT)
            .min(buffers.draco_remaining().saturating_sub(indices));
        // An allowance short of what is read is short of the budget, which charging what is read
        // reports.
        buffers.spend_draco(needed.max(allowance).saturating_add(indices))?;
        let (bytes, _) = buffers.view(Index::new(self.view))?;

        let limits = DecodeLimits::default()
            .with_max_points(count as u64)
            .with_max_faces((index_count / 3) as u64)
            .with_max_decoded_bytes(allowance as u64);
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
        let named: Option<HashSet<u32>> = self
            .attributes
            .values()
            .map(|id| {
                id.as_u64()
                    .and_then(|id| u32::try_from(id).ok())
                    .filter(|&id| mesh.attribute_by_unique_id(id).is_some())
            })
            .collect();
        if named.map(|named| named.len()) != usize::try_from(mesh.num_attributes()).ok() {
            return Err(malformed("holds other attributes than its extension names"));
        }
        Ok(Decoded {
            mesh,
            attributes: self.attributes,
        })
    }
}

impl Decoded<'_> {
    /// The values of the attribute under `semantic`, `N` an element, as floats normalized as
    /// [`Buffers::floats`] normalizes them.
    pub(super) fn vectors<const N: usize>(
        &self,
        semantic: &str,
        accessor: &Accessor,
        normalized: Option<bool>,
    ) -> Result<Vec<[f32; N]>, ModelError> {
        let attribute = self
            .attributes
            .get(semantic)
            .and_then(Value::as_u64)
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
        Ok(values.as_chunks::<N>().0.to_vec())
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
