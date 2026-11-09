use std::{
    collections::{HashMap, hash_map::Entry},
    env,
    path::{Path, PathBuf},
    sync::{LazyLock, RwLock},
};

use anyhow::bail;
use fontdb::Source;
use lumen_engine::{error::MediaError, media::FontResolver};

pub const FONT_DIR_ENV: &str = "LUMEN_FONT_DIR";

/// The installed fonts, scanned the first time a family needs them and shared by every catalog.
static SYSTEM_FONTS: LazyLock<fontdb::Database> = LazyLock::new(|| {
    let mut database = fontdb::Database::new();
    database.load_system_fonts();
    database
});

/// Checks that `value` names a directory, for a flag or variable that says where fonts are. An
/// empty value passes as the empty path, which callers treat as unset.
pub fn existing_font_dir(value: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(value);
    if value.is_empty() || path.is_dir() {
        Ok(path)
    } else {
        Err(format!("`{value}` is not a directory"))
    }
}

/// The directory `LUMEN_FONT_DIR` names, or `None` if it is unset or empty. A directory that
/// doesn't exist is an error.
pub fn font_dir_from_env() -> anyhow::Result<Option<PathBuf>> {
    let Some(value) = env::var_os(FONT_DIR_ENV).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    let Some(value) = value.to_str() else {
        bail!("{FONT_DIR_ENV} is not valid UTF-8");
    };
    match existing_font_dir(value) {
        Ok(dir) => Ok(Some(dir)),
        Err(error) => bail!("{FONT_DIR_ENV}: {error}"),
    }
}

/// Where a family's fonts are looked up, in order: the configured directory, the media root's
/// `assets/fonts`, then the system fonts. A family is matched exactly by the names inside the font
/// files, as shaping does, not by file name, and the first place with a match supplies all of its
/// faces.
pub struct FontCatalog {
    tiers: Vec<FontTier>,
    resolved: RwLock<HashMap<String, Option<ResolvedFont>>>,
}

struct FontTier {
    /// `None` is the system fonts.
    database: Option<fontdb::Database>,
}

impl FontTier {
    fn directory(path: &Path) -> Option<Self> {
        if !path.is_dir() {
            return None;
        }
        let mut database = fontdb::Database::new();
        database.load_fonts_dir(path);
        Some(Self {
            database: Some(database),
        })
    }

    fn files(&self, family: &str) -> Vec<PathBuf> {
        let database = self.database.as_ref().unwrap_or_else(|| &SYSTEM_FONTS);
        let mut files: Vec<PathBuf> = database
            .faces()
            .filter(|face| face.families.iter().any(|(name, _)| name == family))
            .filter_map(|face| match &face.source {
                Source::File(path) | Source::SharedFile(path, _) => Some(path.clone()),
                Source::Binary(_) => None,
            })
            .collect();
        files.sort();
        files.dedup();
        files
    }
}

impl FontCatalog {
    /// A catalog that looks in `configured` first, which must exist, then in `media_root`'s
    /// `assets/fonts` if there is one.
    pub fn new(configured: Option<&Path>, media_root: &Path) -> Self {
        let bundled = media_root.join("assets/fonts");
        Self::in_directories([
            configured,
            (configured != Some(bundled.as_path())).then_some(bundled.as_path()),
        ])
    }

    /// A catalog of `configured` and the system fonts, for families no media root holds.
    pub fn without_media(configured: Option<&Path>) -> Self {
        Self::in_directories([configured])
    }

    fn in_directories<'a>(directories: impl IntoIterator<Item = Option<&'a Path>>) -> Self {
        let mut tiers: Vec<FontTier> = directories
            .into_iter()
            .flatten()
            .filter_map(FontTier::directory)
            .collect();
        tiers.push(FontTier { database: None });
        Self {
            tiers,
            resolved: RwLock::default(),
        }
    }

    /// The fonts for `family`, or `None` if nothing has them, so the text falls back to the
    /// bundled font. `staged` names a file that stands in for the family, and wins over every
    /// directory. The answer is kept, and a family with none is warned about once.
    pub fn resolve(
        &self,
        family: &str,
        staged: impl FnOnce() -> Option<PathBuf>,
    ) -> Option<ResolvedFont> {
        if let Ok(resolved) = self.resolved.read()
            && let Some(font) = resolved.get(family)
        {
            return font.clone();
        }

        let is_bundled_family = family == lumen_text::DEFAULT_FONT_FAMILY;
        let files = match staged() {
            Some(file) => vec![file],
            // An installed Roboto doesn't replace the bundled one, so output stays the same on
            // every machine.
            None => self
                .tiers
                .iter()
                .filter(|tier| tier.database.is_some() || !is_bundled_family)
                .map(|tier| tier.files(family))
                .find(|files| !files.is_empty())
                .unwrap_or_default(),
        };
        let font = (!files.is_empty()).then(|| ResolvedFont::new(family, files));
        if let Ok(mut resolved) = self.resolved.write()
            && let Entry::Vacant(entry) = resolved.entry(family.to_string())
        {
            if font.is_none() && !family.trim().is_empty() && !is_bundled_family {
                tracing::warn!(
                    font_family = family,
                    "no font has this family name, so text using it falls back to other fonts"
                );
            }
            entry.insert(font.clone());
        }
        font
    }
}

/// The font files a family resolved to, named by their current contents.
#[derive(Debug, Clone)]
pub struct ResolvedFont {
    family: String,
    id: String,
    files: Vec<PathBuf>,
}

impl ResolvedFont {
    fn new(family: &str, files: Vec<PathBuf>) -> Self {
        let identities: Vec<String> = files
            .iter()
            .map(|file| {
                let metadata = std::fs::metadata(file).ok();
                let len = metadata.as_ref().map(std::fs::Metadata::len);
                let modified = metadata.and_then(|metadata| metadata.modified().ok());
                format!("{}:{len:?}:{modified:?}", file.display())
            })
            .collect();
        Self {
            family: family.to_string(),
            id: format!("local-font:{}", identities.join(";")),
            files,
        }
    }

    pub fn files(&self) -> &[PathBuf] {
        &self.files
    }
}

impl FontResolver for ResolvedFont {
    fn id(&self) -> &str {
        &self.id
    }

    fn data(&self) -> Result<Vec<Vec<u8>>, MediaError> {
        self.files
            .iter()
            .map(|file| {
                std::fs::read(file).map_err(|err| MediaError::Decode {
                    media_source: self.family.clone(),
                    details: format!("failed reading font data `{}`: {err}", file.display()),
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    const ROBOTO: &[u8] = include_bytes!("../../../text/assets/roboto/Roboto-Regular.ttf");

    fn font_directory(file_name: &str) -> tempfile::TempDir {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join(file_name), ROBOTO).unwrap();
        directory
    }

    fn first_file(catalog: &FontCatalog, family: &str) -> Option<PathBuf> {
        catalog.resolve(family, || None)?.files().first().cloned()
    }

    #[test]
    fn a_family_is_matched_by_the_names_in_the_font_not_the_file_name() {
        let configured = font_directory("not-the-family-name.ttf");
        let root = tempfile::tempdir().unwrap();
        let catalog = FontCatalog::new(Some(configured.path()), root.path());

        let font = catalog.resolve("Roboto", || None).expect("Roboto");
        assert_eq!(font.data().unwrap(), vec![ROBOTO.to_vec()]);
        assert!(catalog.resolve("roboto", || None).is_none());
        assert!(catalog.resolve("not-the-family-name", || None).is_none());
    }

    #[test]
    fn directories_come_before_the_system_fonts_in_order() {
        let configured = font_directory("configured.ttf");
        let root = tempfile::tempdir().unwrap();
        let bundled = root.path().join("assets/fonts");
        fs::create_dir_all(&bundled).unwrap();
        fs::write(bundled.join("bundled.ttf"), ROBOTO).unwrap();

        let both = FontCatalog::new(Some(configured.path()), root.path());
        let directories: Vec<_> = both
            .tiers
            .iter()
            .map(|tier| tier.database.is_some())
            .collect();
        assert_eq!(directories, [true, true, false]);
        assert_eq!(
            first_file(&both, "Roboto"),
            Some(configured.path().join("configured.ttf"))
        );

        let bundled_only = FontCatalog::new(None, root.path());
        assert_eq!(
            first_file(&bundled_only, "Roboto"),
            Some(bundled.join("bundled.ttf"))
        );
        assert_eq!(first_file(&bundled_only, "No Such Family 4f1c"), None);
    }
}
