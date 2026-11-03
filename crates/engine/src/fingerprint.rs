//! Structural hashing for composition cache identity. Not a persistent serialization format.

use std::{
    collections::{BTreeMap, HashMap},
    hash::{BuildHasher, Hash, Hasher},
};

pub use lumen_macros::StructuralHash;

/// Writes content directly into a hasher. Containers include their lengths, enums include
/// their discriminants, and maps visit entries in key order. Floats retain their exact bits.
/// `Node` and `Delegate` derive this automatically for every field.
pub trait StructuralHash {
    fn structural_hash<H: Hasher>(&self, state: &mut H);
}

macro_rules! hash_leaf {
    ($($ty:ty),* $(,)?) => {$(
        impl StructuralHash for $ty {
            fn structural_hash<H: Hasher>(&self, state: &mut H) {
                self.hash(state);
            }
        }
    )*};
}
hash_leaf!(
    bool,
    u8,
    u16,
    u32,
    u64,
    usize,
    i8,
    i16,
    i32,
    i64,
    isize,
    str,
    String,
    crate::expr::BinaryOp,
    crate::expr::UnaryOp,
    crate::expr::GlobalVar,
    crate::expr::BuiltinFn,
);

impl StructuralHash for f32 {
    fn structural_hash<H: Hasher>(&self, state: &mut H) {
        self.to_bits().hash(state);
    }
}

impl StructuralHash for f64 {
    fn structural_hash<H: Hasher>(&self, state: &mut H) {
        self.to_bits().hash(state);
    }
}

impl<T: StructuralHash + ?Sized> StructuralHash for Box<T> {
    fn structural_hash<H: Hasher>(&self, state: &mut H) {
        (**self).structural_hash(state);
    }
}

impl<T: StructuralHash> StructuralHash for [T] {
    fn structural_hash<H: Hasher>(&self, state: &mut H) {
        self.len().hash(state);
        for value in self {
            value.structural_hash(state);
        }
    }
}

impl<T: StructuralHash, const N: usize> StructuralHash for [T; N] {
    fn structural_hash<H: Hasher>(&self, state: &mut H) {
        self.as_slice().structural_hash(state);
    }
}

impl<T: StructuralHash> StructuralHash for Vec<T> {
    fn structural_hash<H: Hasher>(&self, state: &mut H) {
        self.as_slice().structural_hash(state);
    }
}

impl<T: StructuralHash> StructuralHash for Option<T> {
    fn structural_hash<H: Hasher>(&self, state: &mut H) {
        self.is_some().hash(state);
        if let Some(value) = self {
            value.structural_hash(state);
        }
    }
}

impl<A: StructuralHash, B: StructuralHash> StructuralHash for (A, B) {
    fn structural_hash<H: Hasher>(&self, state: &mut H) {
        self.0.structural_hash(state);
        self.1.structural_hash(state);
    }
}

impl<K: StructuralHash, V: StructuralHash> StructuralHash for BTreeMap<K, V> {
    fn structural_hash<H: Hasher>(&self, state: &mut H) {
        self.len().hash(state);
        for (key, value) in self {
            key.structural_hash(state);
            value.structural_hash(state);
        }
    }
}

impl<K: StructuralHash + Ord, V: StructuralHash, S> StructuralHash for HashMap<K, V, S> {
    fn structural_hash<H: Hasher>(&self, state: &mut H) {
        let mut entries = self.iter().collect::<Vec<_>>();
        entries.sort_unstable_by(|(a, _), (b, _)| a.cmp(b));
        entries.len().hash(state);
        for (key, value) in entries {
            key.structural_hash(state);
            value.structural_hash(state);
        }
    }
}

impl StructuralHash for serde_json::Value {
    fn structural_hash<H: Hasher>(&self, state: &mut H) {
        std::mem::discriminant(self).hash(state);
        match self {
            Self::Null => {}
            Self::Bool(value) => value.structural_hash(state),
            Self::Number(value) => {
                // Number's Hash omits its representation tag: e.g. 1.0 and the integer
                // holding its IEEE bits otherwise collide, as do -1 and u64::MAX.
                value.is_u64().hash(state);
                value.is_i64().hash(state);
                // Number's Hash also equates -0.0 with 0.0, which props display differently.
                match value.as_f64().filter(|_| value.is_f64()) {
                    Some(float) => float.to_bits().hash(state),
                    None => value.hash(state),
                }
            }
            Self::String(value) => value.structural_hash(state),
            Self::Array(values) => values.structural_hash(state),
            Self::Object(values) => {
                // Also deterministic when serde_json's `preserve_order` feature is enabled.
                let mut entries = values.iter().collect::<Vec<_>>();
                entries.sort_unstable_by(|(a, _), (b, _)| a.cmp(b));
                entries.len().hash(state);
                for (key, value) in entries {
                    key.structural_hash(state);
                    value.structural_hash(state);
                }
            }
        }
    }
}

pub(crate) fn fingerprint(value: &impl StructuralHash) -> u64 {
    let mut state = foldhash::quality::FixedState::with_seed(0).build_hasher();
    value.structural_hash(&mut state);
    state.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structural_boundaries_and_float_bits_are_preserved() {
        assert_ne!(
            fingerprint(&vec!["ab".to_string(), "c".to_string()]),
            fingerprint(&vec!["a".to_string(), "bc".to_string()])
        );
        assert_ne!(
            fingerprint(&vec![vec![1_u8], vec![2, 3]]),
            fingerprint(&vec![vec![1_u8, 2], vec![3]])
        );
        assert_ne!(
            fingerprint(&serde_json::json!(1.0)),
            fingerprint(&serde_json::json!(1.0_f64.to_bits()))
        );
        assert_ne!(
            fingerprint(&serde_json::json!(-1)),
            fingerprint(&serde_json::json!(u64::MAX))
        );
        assert_ne!(fingerprint(&None::<u8>), fingerprint(&Some(0_u8)));
        assert_ne!(fingerprint(&0.0_f64), fingerprint(&-0.0_f64));
        assert_ne!(
            fingerprint(&serde_json::json!(0.0)),
            fingerprint(&serde_json::json!(-0.0))
        );
        assert_ne!(
            fingerprint(&f32::from_bits(0x7fc00001)),
            fingerprint(&f32::from_bits(0x7fc00002))
        );
        assert_ne!(
            fingerprint(&crate::node::Override::<u8>::Inherit),
            fingerprint(&crate::node::Override::<u8>::Clear)
        );
    }

    #[test]
    fn map_insertion_order_does_not_affect_content_identity() {
        let first = HashMap::from([("a".to_string(), 1_u8), ("b".to_string(), 2)]);
        let mut second = HashMap::with_capacity(128);
        second.insert("b".to_string(), 2_u8);
        second.insert("a".to_string(), 1);
        let ordered = BTreeMap::from([("b".to_string(), 2_u8), ("a".to_string(), 1)]);
        assert_eq!(fingerprint(&first), fingerprint(&second));
        assert_eq!(fingerprint(&first), fingerprint(&ordered));

        let mut first = serde_json::Map::new();
        first.insert("b".to_string(), serde_json::json!([1, true]));
        first.insert("a".to_string(), serde_json::json!("text"));
        let mut second = serde_json::Map::new();
        second.insert("a".to_string(), serde_json::json!("text"));
        second.insert("b".to_string(), serde_json::json!([1, true]));
        assert_eq!(
            fingerprint(&serde_json::Value::Object(first)),
            fingerprint(&serde_json::Value::Object(second))
        );
    }
}
