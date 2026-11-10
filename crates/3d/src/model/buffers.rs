//! A model's buffers, and its accessors read with every offset, count and size checked.

use std::borrow::Cow;
use std::cell::Cell;

use base64::Engine;
use gltf_json::accessor::{ComponentType, Type};
use gltf_json::validation::Checked;
use gltf_json::{Accessor, Index, Root};

use super::{
    MAX_COMPONENTS, MAX_DECODED_BYTES, MAX_VIEW_BYTES, ModelError, invalid, too_large, unsupported,
};

pub(super) struct Buffers<'a> {
    root: &'a Root,
    buffers: Vec<Cow<'a, [u8]>>,
    /// Bytes accessor reads have decoded so far, at four a component.
    decoded: Cell<usize>,
}

/// `value` as a `usize`, or an error naming `what` when it doesn't fit.
pub(super) fn size(value: u64, what: &str) -> Result<usize, ModelError> {
    usize::try_from(value).map_err(|_| invalid(format!("{what} is out of range")))
}

/// The bytes of a base64 `data:` URI, or an error for any other URI, which names a file outside
/// the model.
pub(super) fn data_uri(uri: &str) -> Result<Vec<u8>, ModelError> {
    let Some((header, payload)) = uri
        .strip_prefix("data:")
        .and_then(|rest| rest.split_once(','))
    else {
        return Err(unsupported(format!(
            "the model refers to a file outside itself, `{uri}`; pack everything into one .glb"
        )));
    };
    if !header.ends_with(";base64") {
        return Err(unsupported("a data URI in the model isn't base64"));
    }
    base64::engine::general_purpose::STANDARD
        .decode(payload)
        .map_err(|error| invalid(format!("a data URI in the model is corrupt: {error}")))
}

impl<'a> Buffers<'a> {
    /// The model's buffers: the first may be the file's binary chunk, `bin`, and any may be a
    /// `data:` URI.
    pub(super) fn load(root: &'a Root, bin: Option<&'a [u8]>) -> Result<Self, ModelError> {
        let mut buffers = Vec::with_capacity(root.buffers.len());
        for (index, buffer) in root.buffers.iter().enumerate() {
            let data: Cow<'a, [u8]> = match (&buffer.uri, index, bin) {
                (Some(uri), _, _) => Cow::Owned(data_uri(uri)?),
                (None, 0, Some(bin)) => Cow::Borrowed(bin),
                (None, _, _) => {
                    return Err(invalid(format!(
                        "buffer {index} has neither a URI nor the file's binary chunk"
                    )));
                }
            };
            let length = size(buffer.byte_length.0, "a buffer's length")?;
            if length > data.len() {
                return Err(invalid(format!(
                    "buffer {index} is shorter than its byteLength"
                )));
            }
            buffers.push(match data {
                Cow::Borrowed(data) => Cow::Borrowed(&data[..length]),
                Cow::Owned(mut data) => {
                    data.truncate(length);
                    Cow::Owned(data)
                }
            });
        }
        Ok(Self {
            root,
            buffers,
            decoded: Cell::new(0),
        })
    }

    /// The bytes of buffer view `view`.
    pub(super) fn view(
        &self,
        view: Index<gltf_json::buffer::View>,
    ) -> Result<(&[u8], Option<usize>), ModelError> {
        let json = self
            .root
            .buffer_views
            .get(view.value())
            .ok_or_else(|| invalid("a buffer view index is out of range"))?;
        let buffer = self
            .buffers
            .get(json.buffer.value())
            .ok_or_else(|| invalid("a buffer view names a buffer the model lacks"))?;
        let offset = size(json.byte_offset.map_or(0, |offset| offset.0), "an offset")?;
        let length = size(json.byte_length.0, "a buffer view's length")?;
        let bytes = offset
            .checked_add(length)
            .and_then(|end| buffer.get(offset..end))
            .ok_or_else(|| invalid("a buffer view reaches past the end of its buffer"))?;
        Ok((bytes, json.byte_stride.map(|stride| stride.0)))
    }

    pub(super) fn accessor(&self, index: Index<Accessor>) -> Result<&'a Accessor, ModelError> {
        self.root
            .accessors
            .get(index.value())
            .ok_or_else(|| invalid("an accessor index is out of range"))
    }

    /// Counts `components` toward everything accessors may decode to, failing past it.
    fn spend(&self, components: usize) -> Result<(), ModelError> {
        let total = components
            .checked_mul(4)
            .and_then(|bytes| bytes.checked_add(self.decoded.get()))
            .filter(|&total| total <= MAX_DECODED_BYTES)
            .ok_or_else(|| too_large("bytes of accessor data", MAX_DECODED_BYTES))?;
        self.decoded.set(total);
        Ok(())
    }

    /// An accessor's elements packed tightly, with sparse substitutions applied, and the type of
    /// its components.
    fn dense(&self, accessor: &Accessor) -> Result<(Vec<u8>, ComponentType), ModelError> {
        let (Checked::Valid(component), Checked::Valid(kind)) =
            (&accessor.component_type, &accessor.type_)
        else {
            return Err(invalid("an accessor has an unknown component type or type"));
        };
        let component = component.0;
        let width = component_size(component);
        if matches!(kind, Type::Mat2 | Type::Mat3) && width != 4 {
            return Err(unsupported(
                "an accessor holds a matrix with padded columns",
            ));
        }
        let multiplicity = kind.multiplicity();
        let element = width * multiplicity;
        let count = size(accessor.count.0, "an accessor's count")?;
        let components = count
            .checked_mul(multiplicity)
            .filter(|&components| components <= MAX_COMPONENTS)
            .ok_or_else(|| too_large("components in one accessor", MAX_COMPONENTS))?;
        self.spend(components)?;
        let bytes = sized(count, element)?;
        let source = match accessor.buffer_view {
            Some(view) => {
                let (data, stride) = self.view(view)?;
                let offset = size(
                    accessor.byte_offset.map_or(0, |offset| offset.0),
                    "an offset",
                )?;
                Some(Strided::new(
                    data,
                    offset,
                    stride.unwrap_or(element),
                    element,
                    count,
                )?)
            }
            None => None,
        };
        let sparse = match &accessor.sparse {
            Some(sparse) => {
                let replaced = size(sparse.count.0, "a sparse accessor's count")?;
                if replaced > count {
                    return Err(invalid(
                        "a sparse accessor replaces more elements than it has",
                    ));
                }
                let Checked::Valid(index_type) = &sparse.indices.component_type else {
                    return Err(invalid("a sparse accessor's indices have an unknown type"));
                };
                let index_size = component_size(index_type.0);
                self.spend(replaced.saturating_mul(1 + multiplicity))?;
                let (data, stride) = self.view(sparse.indices.buffer_view)?;
                let offset = size(sparse.indices.byte_offset.0, "an offset")?;
                let indices = Strided::new(
                    data,
                    offset,
                    stride.unwrap_or(index_size),
                    index_size,
                    replaced,
                )?;
                let (data, stride) = self.view(sparse.values.buffer_view)?;
                let offset = size(sparse.values.byte_offset.0, "an offset")?;
                let values =
                    Strided::new(data, offset, stride.unwrap_or(element), element, replaced)?;
                Some((indices, values))
            }
            None => None,
        };

        let mut out = vec![0u8; bytes];
        if let Some(source) = source {
            source.copy_to(&mut out);
        }
        if let Some((indices, values)) = sparse {
            for (index, value) in indices.elements().zip(values.elements()) {
                let index = index
                    .iter()
                    .rev()
                    .fold(0u64, |sum, &byte| (sum << 8) | u64::from(byte));
                let slot = usize::try_from(index)
                    .ok()
                    .and_then(|index| index.checked_mul(element))
                    .and_then(|start| out.get_mut(start..start.checked_add(element)?))
                    .ok_or_else(|| invalid("a sparse accessor replaces an element past its end"))?;
                slot.copy_from_slice(value);
            }
        }
        Ok((out, component))
    }

    /// An accessor's components as floats. Integers scale to 0..1 or -1..1 when normalized, which
    /// `normalized` forces when it is `Some(true)`, and otherwise the accessor's flag decides.
    pub(super) fn floats(
        &self,
        accessor: &Accessor,
        normalized: Option<bool>,
    ) -> Result<Vec<f32>, ModelError> {
        let (bytes, component) = self.dense(accessor)?;
        let normalized = normalized.unwrap_or(accessor.normalized);
        let scale = |value: f32, max: f32| {
            if normalized {
                (value / max).max(-1.0)
            } else {
                value
            }
        };
        Ok(match component {
            ComponentType::F32 => bytes
                .as_chunks::<4>()
                .0
                .iter()
                .map(|b| f32::from_le_bytes(*b))
                .collect(),
            ComponentType::U8 => bytes.iter().map(|&b| scale(f32::from(b), 255.0)).collect(),
            ComponentType::I8 => bytes
                .iter()
                .map(|&b| scale(f32::from(b as i8), 127.0))
                .collect(),
            ComponentType::U16 => bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|b| scale(f32::from(u16::from_le_bytes(*b)), 65535.0))
                .collect(),
            ComponentType::I16 => bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|b| scale(f32::from(i16::from_le_bytes(*b)), 32767.0))
                .collect(),
            ComponentType::U32 => bytes
                .as_chunks::<4>()
                .0
                .iter()
                .map(|b| u32::from_le_bytes(*b) as f32)
                .collect(),
        })
    }

    /// An unsigned integer accessor's components, such as indices or joints.
    fn uints(&self, accessor: &Accessor) -> Result<Vec<u32>, ModelError> {
        let (bytes, component) = self.dense(accessor)?;
        match component {
            ComponentType::U8 => Ok(bytes.iter().map(|&b| u32::from(b)).collect()),
            ComponentType::U16 => Ok(bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|b| u32::from(u16::from_le_bytes(*b)))
                .collect()),
            ComponentType::U32 => Ok(bytes
                .as_chunks::<4>()
                .0
                .iter()
                .map(|b| u32::from_le_bytes(*b))
                .collect()),
            _ => Err(invalid(
                "an index or joint accessor doesn't hold unsigned integers",
            )),
        }
    }

    /// An accessor of `N` components an element, as floats.
    pub(super) fn vectors<const N: usize>(
        &self,
        accessor: &Accessor,
        normalized: Option<bool>,
    ) -> Result<Vec<[f32; N]>, ModelError> {
        check_components(accessor, N)?;
        Ok(self
            .floats(accessor, normalized)?
            .as_chunks::<N>()
            .0
            .to_vec())
    }

    /// An unsigned integer accessor of `N` components an element.
    pub(super) fn uint_vectors<const N: usize>(
        &self,
        accessor: &Accessor,
    ) -> Result<Vec<[u32; N]>, ModelError> {
        check_components(accessor, N)?;
        Ok(self.uints(accessor)?.as_chunks::<N>().0.to_vec())
    }
}

fn check_components(accessor: &Accessor, components: usize) -> Result<(), ModelError> {
    match &accessor.type_ {
        Checked::Valid(kind) if kind.multiplicity() == components => Ok(()),
        _ => Err(invalid(format!(
            "an accessor should have {components} components an element"
        ))),
    }
}

fn component_size(component: ComponentType) -> usize {
    match component {
        ComponentType::I8 | ComponentType::U8 => 1,
        ComponentType::I16 | ComponentType::U16 => 2,
        ComponentType::U32 | ComponentType::F32 => 4,
    }
}

fn sized(count: usize, element: usize) -> Result<usize, ModelError> {
    count
        .checked_mul(element)
        .filter(|&size| size <= MAX_VIEW_BYTES)
        .ok_or_else(|| too_large("bytes in one accessor", MAX_VIEW_BYTES))
}

/// `count` elements of `element` bytes that sit `stride` bytes apart from `offset` in `data`,
/// checked to lie within it.
struct Strided<'a> {
    data: &'a [u8],
    offset: usize,
    stride: usize,
    element: usize,
    count: usize,
}

impl<'a> Strided<'a> {
    fn new(
        data: &'a [u8],
        offset: usize,
        stride: usize,
        element: usize,
        count: usize,
    ) -> Result<Self, ModelError> {
        if count > 0 {
            if stride < element {
                return Err(invalid(
                    "a buffer view's stride is smaller than its elements",
                ));
            }
            let end = (count - 1)
                .checked_mul(stride)
                .and_then(|span| span.checked_add(offset))
                .and_then(|start| start.checked_add(element));
            if end.is_none_or(|end| end > data.len()) {
                return Err(invalid(
                    "an accessor reaches past the end of its buffer view",
                ));
            }
        }
        Ok(Self {
            data,
            offset,
            stride,
            element,
            count,
        })
    }

    fn elements(&self) -> impl Iterator<Item = &'a [u8]> + '_ {
        (0..self.count).map(|i| {
            let start = self.offset + i * self.stride;
            &self.data[start..start + self.element]
        })
    }

    fn copy_to(&self, out: &mut [u8]) {
        for (chunk, element) in out.chunks_exact_mut(self.element).zip(self.elements()) {
            chunk.copy_from_slice(element);
        }
    }
}
