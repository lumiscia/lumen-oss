//! A pass over the JSON chunk before it is deserialised. It walks the document without keeping
//! any of it, so it can bound what deserialising allocates: every value counts toward
//! [`MAX_JSON_VALUES`], and each collection toward its own limit. It also rejects integers wider
//! than 32 bits where `gltf-json` reads 32 bits by truncating: indices and the enumerated codes
//! such as component types. Sizes and counts are read as 64 bits and bounded later.

use std::fmt;

use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};

use super::{MAX_JSON_BYTES, MAX_JSON_VALUES, MAX_NODES, ModelError, invalid, too_large};

/// The most elements each top-level collection may hold, by its key.
pub const COLLECTION_LIMITS: &[(&str, usize)] = &[
    ("accessors", 200_000),
    ("animations", 10_000),
    ("bufferViews", 200_000),
    ("buffers", 1_024),
    ("cameras", 10_000),
    ("images", 10_000),
    ("materials", 10_000),
    ("meshes", 100_000),
    ("nodes", MAX_NODES),
    ("samplers", 10_000),
    ("scenes", 1_000),
    ("skins", 1_024),
    ("textures", 10_000),
];
/// The most primitives all meshes may hold together.
pub const MAX_PRIMITIVES: usize = 100_000;
/// The most channels, and the most samplers, one animation may hold.
pub const MAX_ANIMATION_CHANNELS: usize = 100_000;

/// Keys whose integers, or the integers of whose arrays, `gltf-json` truncates to 32 bits: the
/// indices, then the enumerated codes. Under some of them other places hold objects instead,
/// such as the sparse `indices` of an accessor or a channel's `target`, whose own keys are then
/// told apart in turn.
const NARROW_KEYS: &[&str] = &[
    "bufferView",
    "buffer",
    "camera",
    "children",
    "index",
    "indices",
    "input",
    "inverseBindMatrices",
    "joints",
    "material",
    "mesh",
    "node",
    "nodes",
    "output",
    "sampler",
    "scene",
    "skeleton",
    "skin",
    "source",
    "componentType",
    "magFilter",
    "minFilter",
    "mode",
    "target",
    "wrapS",
    "wrapT",
];
/// Keys of objects every value of which is an index, or of arrays of them: a primitive's
/// `attributes` and morph `targets`.
const INDEX_MAP_KEYS: &[&str] = &["attributes", "targets"];

/// Checks `json` as the module comment says, before it is deserialised.
pub(super) fn check(json: &[u8]) -> Result<(), ModelError> {
    if json.len() > MAX_JSON_BYTES {
        return Err(too_large("bytes of JSON", MAX_JSON_BYTES));
    }
    let mut scan = Scan::default();
    let mut deserializer = serde_json::Deserializer::from_slice(json);
    let walked = Walk {
        scan: &mut scan,
        place: Place::Document,
    }
    .deserialize(&mut deserializer)
    .and_then(|()| deserializer.end());
    match (scan.failure, walked) {
        (Some(failure), _) => Err(failure),
        (None, Err(error)) => Err(invalid(format!(
            "the model's JSON isn't valid glTF: {error}"
        ))),
        (None, Ok(())) => Ok(()),
    }
}

#[derive(Default)]
struct Scan {
    values: usize,
    primitives: usize,
    /// Why the walk stopped, when it was this check rather than the JSON's syntax.
    failure: Option<ModelError>,
}

#[derive(Clone, Copy)]
enum Place {
    Document,
    /// An array of `limit` elements at most, each at `item`.
    Collection {
        what: &'static str,
        limit: usize,
        item: Item,
    },
    Item(Item),
    /// Where an integer is read as 32 bits, truncated.
    Narrow,
    /// An object whose values are [`Place::Narrow`], or an array of them.
    NarrowValues,
    /// Anywhere else in core glTF.
    Core,
    /// Within `extras` or `extensions`, where nothing is read as an index or size.
    Free,
}

#[derive(Clone, Copy)]
enum Item {
    Mesh,
    Primitive,
    Animation,
    Other,
}

struct Walk<'s> {
    scan: &'s mut Scan,
    place: Place,
}

impl Walk<'_> {
    fn fail<E: de::Error>(&mut self, failure: ModelError) -> E {
        self.scan.failure = Some(failure);
        E::custom("stopped")
    }

    fn count<E: de::Error>(&mut self) -> Result<(), E> {
        self.scan.values += 1;
        if self.scan.values > MAX_JSON_VALUES {
            return Err(self.fail(too_large("JSON values", MAX_JSON_VALUES)));
        }
        Ok(())
    }

    /// Where the value under `key` of an object at this place is.
    fn child(&self, key: &str) -> Place {
        if matches!(self.place, Place::Free) || matches!(key, "extras" | "extensions") {
            return Place::Free;
        }
        if matches!(self.place, Place::NarrowValues) {
            return Place::Narrow;
        }
        if matches!(self.place, Place::Document)
            && let Some(&(what, limit)) = COLLECTION_LIMITS.iter().find(|(name, _)| *name == key)
        {
            let item = match key {
                "meshes" => Item::Mesh,
                "animations" => Item::Animation,
                _ => Item::Other,
            };
            return Place::Collection { what, limit, item };
        }
        match (self.place, key) {
            (Place::Item(Item::Mesh), "primitives") => Place::Collection {
                what: "primitives",
                limit: MAX_PRIMITIVES,
                item: Item::Primitive,
            },
            (Place::Item(Item::Animation), "channels") => Place::Collection {
                what: "channels in one animation",
                limit: MAX_ANIMATION_CHANNELS,
                item: Item::Other,
            },
            (Place::Item(Item::Animation), "samplers") => Place::Collection {
                what: "samplers in one animation",
                limit: MAX_ANIMATION_CHANNELS,
                item: Item::Other,
            },
            (_, key) if INDEX_MAP_KEYS.contains(&key) => Place::NarrowValues,
            (_, key) if NARROW_KEYS.contains(&key) => Place::Narrow,
            _ => Place::Core,
        }
    }
}

impl<'de> DeserializeSeed<'de> for Walk<'_> {
    type Value = ();

    fn deserialize<D: de::Deserializer<'de>>(self, deserializer: D) -> Result<(), D::Error> {
        deserializer.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for Walk<'_> {
    type Value = ();

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("any JSON value")
    }

    fn visit_bool<E: de::Error>(mut self, _: bool) -> Result<(), E> {
        self.count()
    }

    fn visit_i64<E: de::Error>(mut self, _: i64) -> Result<(), E> {
        self.count()
    }

    fn visit_u64<E: de::Error>(mut self, value: u64) -> Result<(), E> {
        self.count()?;
        if value > u64::from(u32::MAX) && matches!(self.place, Place::Narrow) {
            return Err(self.fail(invalid(format!(
                "the model's JSON holds {value} where an index or code belongs, which 32 bits \
                 can't hold"
            ))));
        }
        Ok(())
    }

    fn visit_f64<E: de::Error>(mut self, _: f64) -> Result<(), E> {
        self.count()
    }

    fn visit_str<E: de::Error>(mut self, _: &str) -> Result<(), E> {
        self.count()
    }

    fn visit_unit<E: de::Error>(mut self) -> Result<(), E> {
        self.count()
    }

    fn visit_seq<A: SeqAccess<'de>>(mut self, mut seq: A) -> Result<(), A::Error> {
        self.count()?;
        let (limit, item) = match self.place {
            Place::Collection { what, limit, item } => (Some((what, limit)), Place::Item(item)),
            place @ (Place::Free | Place::Narrow | Place::NarrowValues) => (None, place),
            _ => (None, Place::Core),
        };
        let mut elements = 0usize;
        loop {
            let walk = Walk {
                scan: &mut *self.scan,
                place: item,
            };
            if seq.next_element_seed(walk)?.is_none() {
                return Ok(());
            }
            elements += 1;
            if let Some((what, limit)) = limit
                && elements > limit
            {
                return Err(self.fail(too_large(what, limit)));
            }
            if matches!(item, Place::Item(Item::Primitive)) {
                self.scan.primitives += 1;
                if self.scan.primitives > MAX_PRIMITIVES {
                    return Err(self.fail(too_large("primitives", MAX_PRIMITIVES)));
                }
            }
        }
    }

    fn visit_map<A: MapAccess<'de>>(mut self, mut map: A) -> Result<(), A::Error> {
        self.count()?;
        while let Some(key) = map.next_key_seed(Key)? {
            self.count()?;
            let place = self.child(key);
            map.next_value_seed(Walk {
                scan: &mut *self.scan,
                place,
            })?;
        }
        Ok(())
    }
}

/// An object's key, kept only when it is one this walk tells apart from the rest.
struct Key;

impl<'de> DeserializeSeed<'de> for Key {
    type Value = &'static str;

    fn deserialize<D: de::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> Result<Self::Value, D::Error> {
        deserializer.deserialize_str(self)
    }
}

impl Visitor<'_> for Key {
    type Value = &'static str;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("an object key")
    }

    fn visit_str<E: de::Error>(self, key: &str) -> Result<Self::Value, E> {
        const KEYS: &[&str] = &["extras", "extensions", "primitives", "channels", "samplers"];
        Ok(COLLECTION_LIMITS
            .iter()
            .map(|(name, _)| *name)
            .chain(
                KEYS.iter()
                    .chain(NARROW_KEYS)
                    .chain(INDEX_MAP_KEYS)
                    .copied(),
            )
            .find(|name| *name == key)
            .unwrap_or(""))
    }
}
