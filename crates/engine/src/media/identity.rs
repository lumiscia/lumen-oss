//! Which bytes a store serves for a source, so that what the engine keeps of them across frames
//! and renders is used again only while they stay the same.

use std::fmt::Write;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::UNIX_EPOCH;

/// The bytes a resolved source holds. Two equal identities, from any store in the process, hold
/// the same bytes, so the engine loads, decodes and uploads media once per identity and keeps the
/// result for as long as its caches allow. A host gives media a new identity whenever its bytes
/// change, or the engine keeps showing the old ones.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MediaIdentity(Arc<str>);

impl MediaIdentity {
    /// A file, by its path, length and modification time, which change when it is replaced or
    /// written. A file that can't be read is named by its path alone.
    pub fn of_file(path: impl AsRef<Path>) -> Self {
        Self::of_files([path])
    }

    /// Several files read as one source, such as a font family's faces, as [`Self::of_file`]
    /// names each.
    pub fn of_files(paths: impl IntoIterator<Item = impl AsRef<Path>>) -> Self {
        let mut key = String::from("file:");
        for path in paths {
            let path = path.as_ref();
            match std::fs::metadata(path) {
                Ok(metadata) => {
                    let modified = metadata
                        .modified()
                        .ok()
                        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                        .map(|since| since.as_nanos());
                    let _ = write!(key, "{}:{modified:?}:{path:?};", metadata.len());
                }
                Err(_) => {
                    let _ = write!(key, "-:{path:?};");
                }
            }
        }
        Self(key.into())
    }

    /// Bytes that `key` always names, such as a content hash or the id of an upload that never
    /// changes.
    pub fn of_key(key: &str) -> Self {
        Self(format!("key:{key}").into())
    }

    /// An identity no other equals, for bytes a host was handed in memory. Take a new one each
    /// time they are set.
    pub fn unique() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        Self(format!("#{}", NEXT.fetch_add(1, Ordering::Relaxed)).into())
    }

    /// The identity as a string, equal for equal identities only.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_has_a_new_identity_once_it_is_written() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("logo.png");
        let missing = MediaIdentity::of_file(&path);
        assert_eq!(MediaIdentity::of_file(&path), missing);
        std::fs::write(&path, b"one").unwrap();
        let written = MediaIdentity::of_file(&path);
        assert_ne!(written, missing);
        assert_eq!(MediaIdentity::of_file(&path), written);
        std::fs::write(&path, b"other").unwrap();
        assert_ne!(MediaIdentity::of_file(&path), written);
    }

    #[test]
    fn identities_of_different_origins_never_meet() {
        assert_ne!(MediaIdentity::unique(), MediaIdentity::unique());
        assert_eq!(MediaIdentity::of_key("a"), MediaIdentity::of_key("a"));
        assert_ne!(MediaIdentity::of_key("#0"), MediaIdentity(Arc::from("#0")));
        assert_ne!(
            MediaIdentity::of_key("file:"),
            MediaIdentity::of_files(std::iter::empty::<&Path>())
        );
    }
}
