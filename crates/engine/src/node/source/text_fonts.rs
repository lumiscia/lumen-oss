use std::{
    collections::HashMap,
    sync::{Arc, LazyLock, Mutex, Weak},
};

use crate::error::{LumenError, MediaFailure, MediaReason, RenderError};
use crate::media::{
    AssetKind, FontResolver, MediaIdentity, MediaStore, TextSession, asset_id, resolve_as, sniff,
};

use super::text::TextLayoutParams;

/// Loads the families `layout` and its spans name.
pub(crate) fn load_font_families(
    text_system: &mut lumen_text::TextSystem,
    media: Option<&dyn MediaStore>,
    layout: &TextLayoutParams,
) -> crate::Result<()> {
    for family in used_font_families(layout) {
        load_font_family(text_system, media, family)?;
    }
    Ok(())
}

/// The families text in `layout` is shaped in: each span's, and the node's when a span leaves its
/// own unset.
pub(crate) fn used_font_families(layout: &TextLayoutParams) -> impl Iterator<Item = &str> {
    let spans = &layout.content.0;
    let inherits = spans.is_empty() || spans.iter().any(|span| span.font_family.is_none());
    inherits
        .then_some(layout.font_family.as_str())
        .into_iter()
        .chain(spans.iter().filter_map(|span| span.font_family.as_deref()))
}

/// The text system of `media`'s fonts, which rendering and text measurement share so both see
/// them. Do not hold the guard while evaluating expressions, which may measure text.
pub(crate) fn text_system<'a>(
    media: Option<&'a dyn MediaStore>,
) -> crate::Result<std::sync::MutexGuard<'a, lumen_text::TextSystem>> {
    static SHARED: LazyLock<TextSession> = LazyLock::new(TextSession::default);
    media
        .and_then(|store| store.text_session())
        .unwrap_or(&SHARED)
        .lock()
}

pub(crate) fn load_font_family(
    text_system: &mut lumen_text::TextSystem,
    media: Option<&dyn MediaStore>,
    font_family: &str,
) -> crate::Result<()> {
    if font_family.is_empty() {
        return Ok(());
    }
    let Some(store) = media else {
        return Ok(());
    };
    let (identity, resolver) = match resolve_as::<dyn FontResolver>(store, font_family) {
        Ok(resolved) => resolved,
        // A family the store doesn't have falls back to other fonts, unless it names an asset.
        Err(error)
            if asset_id(font_family).is_none()
                && error.failure().is_some_and(|failure| {
                    matches!(failure.reason, MediaReason::NotFound { .. })
                }) =>
        {
            return Ok(());
        }
        Err(error) => return Err(LumenError::Media(error)),
    };
    if text_system.family_resolver(font_family) == Some(identity.as_str()) {
        return Ok(());
    }
    let files = font_files(font_family, &identity, &*resolver)?;
    let heads: Vec<Vec<u8>> = files
        .iter()
        .map(|file| file.iter().take(16).copied().collect())
        .collect();
    text_system
        .load_family(font_family, identity.as_str(), files)
        .map_err(|_| LumenError::Media(unreadable_font(font_family, &heads).into()))?;
    Ok(())
}

/// Why the files of `family`, whose first bytes are `heads`, hold no readable face.
fn unreadable_font(family: &str, heads: &[Vec<u8>]) -> MediaFailure {
    let wanted = [AssetKind::Font];
    for head in heads {
        if head.starts_with(b"wOFF") || head.starts_with(b"wOF2") {
            return MediaFailure::unsupported(
                family,
                "WOFF fonts are not loaded; use a TrueType or OpenType file",
            )
            .wanting(&wanted);
        }
        if let Some(found) = sniff(head).other_than(AssetKind::Font) {
            return MediaFailure::wrong_kind(family, &wanted, found, "the file is not a font");
        }
    }
    MediaFailure::decode(family, "its font data holds no readable face").wanting(&wanted)
}

/// The data of `resolver`'s font files, shared with every text system still holding that family
/// and identity's, so that jobs using one font hold and read it once.
fn font_files(
    family: &str,
    identity: &MediaIdentity,
    resolver: &dyn FontResolver,
) -> crate::Result<Vec<Arc<Vec<u8>>>> {
    type Files = HashMap<(String, String), Vec<Weak<Vec<u8>>>>;
    static FILES: LazyLock<Mutex<Files>> = LazyLock::new(Mutex::default);
    let lock = || {
        FILES.lock().map_err(|_| {
            LumenError::Render(RenderError::Internal {
                details: "font file cache lock was poisoned".to_string(),
            })
        })
    };
    let key = (family.to_string(), identity.as_str().to_string());
    if let Some(files) = lock()?
        .get(&key)
        .and_then(|files| files.iter().map(Weak::upgrade).collect())
    {
        return Ok(files);
    }
    let files: Vec<_> = resolver
        .data()
        .map_err(LumenError::Media)?
        .into_iter()
        .map(Arc::new)
        .collect();
    let mut cache = lock()?;
    cache.retain(|_, files| files.iter().all(|file| file.strong_count() > 0));
    cache.insert(key, files.iter().map(Arc::downgrade).collect());
    Ok(files)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;
    use crate::error::MediaError;
    use crate::media::{MediaResolver, ResolvedMedia};

    #[test]
    fn a_font_file_that_cant_be_read_fails_by_what_the_file_is() {
        let png = vec![b"\x89PNG\r\n\x1a\n".to_vec()];
        let cases = [
            (vec![b"wOFF\0\x01\0\0".to_vec()], "media.unsupported", None),
            (vec![b"wOF2".to_vec()], "media.unsupported", None),
            (png, "media.wrong_kind", Some(AssetKind::Image)),
            (vec![b"plain text".to_vec()], "media.decode", None),
        ];
        for (heads, code, found) in cases {
            let failure = unreadable_font("Brand", &heads);
            assert_eq!(failure.code(), code, "{failure}");
            assert_eq!(failure.found, found, "{failure}");
            assert_eq!(failure.wanted, [AssetKind::Font]);
            assert_eq!(failure.source, "Brand");
        }
    }

    /// Serves Inter as "Known", or every family if `every_family`, all as one identity.
    #[derive(Debug, Default)]
    struct CountingStore {
        every_family: bool,
        reads: Arc<AtomicU64>,
    }

    impl CountingStore {
        /// An identity of this store alone, so tests running together share no font.
        fn identity(&self) -> MediaIdentity {
            MediaIdentity::of_key(&format!("counting-store-{:p}", Arc::as_ptr(&self.reads)))
        }
    }

    struct CountingResolver {
        family: String,
        reads: Arc<AtomicU64>,
    }

    impl FontResolver for CountingResolver {
        fn data(&self) -> Result<Vec<Vec<u8>>, MediaError> {
            self.reads.fetch_add(1, Ordering::Relaxed);
            Ok(vec![match self.family.as_str() {
                "Known" | "Inter" => INTER.to_vec(),
                _ => lumen_text::ROBOTO_REGULAR_BYTES.to_vec(),
            }])
        }
    }

    impl MediaStore for CountingStore {
        fn resolve(&self, family: &str, kind: AssetKind) -> Result<ResolvedMedia, MediaError> {
            if kind != AssetKind::Font || !(self.every_family || family == "Known") {
                return Err(MediaError::source_not_found(family, "not served"));
            }
            Ok(ResolvedMedia::new(
                self.identity(),
                MediaResolver::Font(Box::new(CountingResolver {
                    family: family.to_string(),
                    reads: Arc::clone(&self.reads),
                })),
            ))
        }
    }

    const INTER: &[u8] = include_bytes!("../../../../text/assets/inter/InterVariable-Latin.ttf");

    #[test]
    fn a_font_loads_once_per_identity_and_a_missing_one_falls_back() {
        let store = CountingStore::default();
        let mut system = lumen_text::TextSystem::new();
        for _ in 0..3 {
            load_font_family(&mut system, Some(&store), "Known").unwrap();
            load_font_family(&mut system, Some(&store), "Unknown").unwrap();
        }
        assert_eq!(
            system.family_resolver("Known"),
            Some(store.identity().as_str())
        );
        assert_eq!(system.family_resolver("Unknown"), None);
        assert_eq!(store.reads.load(Ordering::Relaxed), 1);

        let missing = load_font_family(&mut system, Some(&store), "asset:brand").unwrap_err();
        assert_eq!(missing.code(), "media.not_found");
    }

    #[test]
    fn an_identity_only_shares_a_font_within_its_family() {
        let store = CountingStore {
            every_family: true,
            ..Default::default()
        };
        let mut system = lumen_text::TextSystem::new();
        load_font_family(&mut system, Some(&store), "Inter").unwrap();
        load_font_family(&mut system, Some(&store), "Other").unwrap();
        assert!(system.has_family("Inter"));
        assert_eq!(store.reads.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn text_systems_share_a_font_while_one_holds_it() {
        let store = CountingStore::default();
        let reads = || store.reads.load(Ordering::Relaxed);
        let load = |system: &mut lumen_text::TextSystem| {
            load_font_family(system, Some(&store), "Known").unwrap();
        };

        let mut first = lumen_text::TextSystem::new();
        load(&mut first);
        load(&mut lumen_text::TextSystem::new());
        assert_eq!(reads(), 1);

        drop(first);
        load(&mut lumen_text::TextSystem::new());
        assert_eq!(reads(), 2);
    }
}
