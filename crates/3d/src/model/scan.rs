//! A pass over the JSON chunk before it is deserialised. It walks the document without keeping
//! any of it, so it can bound what deserialising allocates: every value counts toward
//! [`MAX_JSON_VALUES`], and each collection toward its own limit. It also rejects integers wider
//! than 32 bits outside `extras` and `extensions`, which no index or size can hold and which
//! deserialising would truncate.

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
    /// Core glTF, whose integers must fit in 32 bits.
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
        match (self.place, key) {
            (Place::Free, _) | (_, "extras" | "extensions") => Place::Free,
            (Place::Document, key) => COLLECTION_LIMITS
                .iter()
                .find(|(name, _)| *name == key)
                .map_or(Place::Core, |&(what, limit)| Place::Collection {
                    what,
                    limit,
                    item: match key {
                        "meshes" => Item::Mesh,
                        "animations" => Item::Animation,
                        _ => Item::Other,
                    },
                }),
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
        if value > u64::from(u32::MAX) && !matches!(self.place, Place::Free) {
            return Err(self.fail(invalid(format!(
                "the model's JSON holds {value}, larger than any index or size in a model"
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
            Place::Free => (None, Place::Free),
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
            .chain(KEYS.iter().copied())
            .find(|name| *name == key)
            .unwrap_or(""))
    }
}
