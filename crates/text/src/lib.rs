//! Text layout and GPU-facing text data for Lumen: shaping, measurement, atlas bookkeeping and
//! WGSL-facing buffer formats.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use cosmic_text::{FontSystem, SwashCache, fontdb};

mod atlas;
mod decorate;
mod gpu;
mod msdf;
mod shape;
mod transform;

pub use atlas::{GlyphAtlas, GlyphUpload, PreparedGlyphs, TextRenderMode, stroke_reach};
pub use decorate::{
    TextDecoration, TextDecorationKind, TextLineDecoration, TextLineStyle, TextSpanMetrics,
};
pub use gpu::*;
pub use msdf::MSDF_PX_RANGE;
pub use shape::{
    FontFeature, GlyphKey, ShapedText, TextAlign, TextFontStyle, TextGlyph, TextItem, TextLayout,
    TextLayoutRequest, TextMeasurement, TextOverflow, TextSegment, TextSpan,
};
pub use transform::TextTransform;

/// Text shader body; the engine prepends its alpha and paint WGSL chunks.
pub const TEXT_SHADER: &str = include_str!("shaders/text.wgsl");
pub const DEFAULT_FONT_FAMILY: &str = "Roboto";
pub const ROBOTO_REGULAR_BYTES: &[u8] = include_bytes!("../assets/roboto/Roboto-Regular.ttf");

/// Uses no platform font lists, so fallback chooses among the loaded fonts the same way on every
/// platform.
struct NoPlatformFallback;

impl cosmic_text::Fallback for NoPlatformFallback {
    fn common_fallback(&self) -> &[&'static str] {
        &[]
    }

    fn forbidden_fallback(&self) -> &[&'static str] {
        &[]
    }

    fn script_fallback(&self, _: unicode_script::Script, _: &str) -> &[&'static str] {
        &[]
    }
}

fn new_font_system(db: fontdb::Database) -> FontSystem {
    FontSystem::new_with_locale_and_db_and_fallback("en-US".to_string(), db, NoPlatformFallback)
}

/// A generation no text system has used.
fn next_font_generation() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// Fonts loaded under one name, or under none by `load_font_data`.
#[derive(Debug)]
struct Registration {
    family: Option<String>,
    resolver_id: String,
    files: Vec<Arc<Vec<u8>>>,
}

/// A database of every registered face, latest first and the bundled ones last. Faces tying for a
/// request go to the earliest in a database, so the latest load wins.
fn font_database(registrations: &[Registration]) -> fontdb::Database {
    let mut db = fontdb::Database::new();
    for file in registrations.iter().flat_map(|loaded| &loaded.files) {
        db.load_font_source(fontdb::Source::Binary(file.clone()));
    }
    db.load_font_source(fontdb::Source::Binary(Arc::new(ROBOTO_REGULAR_BYTES)));
    db
}

/// Font data that holds no readable face.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoFontFaces;

#[derive(Debug)]
pub struct TextSystem {
    font_system: FontSystem,
    swash_cache: SwashCache,
    font_generation: u64,
    /// Latest first.
    registrations: Vec<Registration>,
    /// Recently shaped requests, newest last.
    shapes: Vec<(TextLayoutRequest, Arc<ShapedText>)>,
}

impl TextSystem {
    /// A text system with only the bundled fonts. Other fonts must be loaded with
    /// `load_family` or `load_font_data`, so text renders the same on every machine.
    pub fn new() -> Self {
        Self {
            font_system: new_font_system(font_database(&[])),
            swash_cache: SwashCache::new(),
            font_generation: next_font_generation(),
            registrations: Vec::new(),
            shapes: Vec::new(),
        }
    }

    pub fn load_font_data(&mut self, data: Vec<u8>) {
        self.registrations.insert(
            0,
            Registration {
                family: None,
                resolver_id: String::new(),
                files: vec![Arc::new(data)],
            },
        );
        self.rebuild_fonts();
    }

    fn registration(&self, family: &str) -> Option<usize> {
        self.registrations
            .iter()
            .position(|loaded| loaded.family.as_deref() == Some(family))
    }

    /// The resolver `family` was last loaded from.
    pub fn family_resolver(&self, family: &str) -> Option<&str> {
        let index = self.registration(family)?;
        Some(&self.registrations[index].resolver_id)
    }

    /// Whether any loaded face answers to `family`.
    pub fn has_family(&self, family: &str) -> bool {
        self.font_system
            .db()
            .faces()
            .any(|face| face.families.iter().any(|(name, _)| name == family))
    }

    /// Makes `files` what is loaded under the name `family`, in place of what was. Every face
    /// loaded under any name stays available to text that needs its glyphs, and the bundled ones
    /// are always there. Faces tying for a request go to the latest load. On failure, or with no
    /// files, what `family` has stays. Replaced data is freed once no other text system holds it.
    pub fn load_family(
        &mut self,
        family: &str,
        resolver_id: &str,
        files: Vec<Arc<Vec<u8>>>,
    ) -> Result<(), NoFontFaces> {
        let current = self.registration(family);
        if files.is_empty() {
            match current {
                Some(index) => self.registrations[index].resolver_id = resolver_id.to_string(),
                None => self.registrations.push(Registration {
                    family: Some(family.to_string()),
                    resolver_id: resolver_id.to_string(),
                    files,
                }),
            }
            return Ok(());
        }
        let mut probe = fontdb::Database::new();
        for file in &files {
            if probe
                .load_font_source(fontdb::Source::Binary(file.clone()))
                .is_empty()
            {
                return Err(NoFontFaces);
            }
        }
        if let Some(index) = current {
            self.registrations.remove(index);
        }
        self.registrations.insert(
            0,
            Registration {
                family: Some(family.to_string()),
                resolver_id: resolver_id.to_string(),
                files,
            },
        );
        self.rebuild_fonts();
        Ok(())
    }

    /// Removes what is loaded under the name `family`. Text asking for it uses the faces that
    /// remain.
    pub fn remove_family(&mut self, family: &str) {
        if let Some(index) = self.registration(family) {
            self.registrations.remove(index);
            self.rebuild_fonts();
        }
    }

    /// Replaces everything derived from the fonts. Font IDs number the faces of one database, so
    /// nothing keyed by them may outlive it.
    fn rebuild_fonts(&mut self) {
        self.font_system = new_font_system(font_database(&self.registrations));
        self.font_generation = next_font_generation();
        self.shapes.clear();
    }

    /// Changes whenever the fonts do and differs between text systems. What is keyed by font IDs
    /// or shaped from fonts is only good for the generation it came from.
    pub fn font_generation(&self) -> u64 {
        self.font_generation
    }
}

impl Default for TextSystem {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn loads_only_the_bundled_fonts() {
        assert_eq!(TextSystem::new().font_system.db().len(), 1);
    }

    const INTER: &[u8] = include_bytes!("../assets/inter/InterVariable-Latin.ttf");

    /// Roboto under its own name, drawn at `1 / scale` of its size by a larger em square.
    pub(crate) fn scaled_roboto(scale: u16) -> Arc<Vec<u8>> {
        scaled(ROBOTO_REGULAR_BYTES, scale)
    }

    /// `font` drawn at `1 / scale` of its size by a larger em square.
    fn scaled(font: &[u8], scale: u16) -> Arc<Vec<u8>> {
        let mut font = font.to_vec();
        let tables = u16::from_be_bytes([font[4], font[5]]) as usize;
        let head = font[12..12 + tables * 16]
            .chunks(16)
            .find(|table| &table[..4] == b"head")
            .unwrap();
        let units = u32::from_be_bytes(head[8..12].try_into().unwrap()) as usize + 18;
        let value = u16::from_be_bytes([font[units], font[units + 1]]);
        font[units..units + 2].copy_from_slice(&(value * scale).to_be_bytes());
        Arc::new(font)
    }

    #[test]
    fn replacing_a_family_changes_its_text_and_frees_the_old_font() {
        let mut system = TextSystem::new();
        let request = TextLayoutRequest::new("WWW");
        let bundled = system.measure(&request).width;

        let half = scaled_roboto(2);
        let half_data = Arc::downgrade(&half);
        system
            .load_family(DEFAULT_FONT_FAMILY, "half", vec![half])
            .unwrap();
        assert_eq!(system.family_resolver(DEFAULT_FONT_FAMILY), Some("half"));
        assert!((system.measure(&request).width * 2.0 - bundled).abs() < 1.0);
        assert!(half_data.upgrade().is_some());

        system
            .load_family(DEFAULT_FONT_FAMILY, "quarter", vec![scaled_roboto(4)])
            .unwrap();
        assert!((system.measure(&request).width * 4.0 - bundled).abs() < 1.0);
        assert_eq!(system.font_system.db().len(), 2);
        assert!(half_data.upgrade().is_none());
    }

    /// `font` declaring the weight `weight`.
    fn with_weight(font: &[u8], weight: u16) -> Arc<Vec<u8>> {
        let mut font = font.to_vec();
        let tables = u16::from_be_bytes([font[4], font[5]]) as usize;
        let os2 = font[12..12 + tables * 16]
            .chunks(16)
            .find(|table| &table[..4] == b"OS/2")
            .unwrap();
        let at = u32::from_be_bytes(os2[8..12].try_into().unwrap()) as usize + 4;
        font[at..at + 2].copy_from_slice(&weight.to_be_bytes());
        Arc::new(font)
    }

    #[test]
    fn replacing_a_family_outranks_faces_another_name_brought() {
        let mut system = TextSystem::new();
        let request = TextLayoutRequest::new("WWW");
        let bundled = system.measure(&request).width;

        // A font asked for by path goes by its internal name, not the one it was asked for by.
        system
            .load_family(DEFAULT_FONT_FAMILY, "half", vec![scaled_roboto(2)])
            .unwrap();
        system
            .load_family("fonts/Roboto.ttf", "third", vec![scaled_roboto(3)])
            .unwrap();
        assert!((system.measure(&request).width * 3.0 - bundled).abs() < 1.0);
        system
            .load_family(DEFAULT_FONT_FAMILY, "quarter", vec![scaled_roboto(4)])
            .unwrap();
        assert!((system.measure(&request).width * 4.0 - bundled).abs() < 1.0);

        // What was loaded under another name stays, as does the resolver it remembers.
        assert_eq!(system.font_system.db().len(), 3);
        assert_eq!(system.family_resolver("fonts/Roboto.ttf"), Some("third"));
    }

    /// `fonts` as one font collection.
    fn collection(fonts: &[&[u8]]) -> Arc<Vec<u8>> {
        let mut data = b"ttcf\0\x01\0\0".to_vec();
        data.extend((fonts.len() as u32).to_be_bytes());
        data.resize(12 + fonts.len() * 4, 0);
        for (index, font) in fonts.iter().enumerate() {
            data.resize(data.len().next_multiple_of(4), 0);
            let start = data.len() as u32;
            data[12 + index * 4..16 + index * 4].copy_from_slice(&start.to_be_bytes());
            data.extend_from_slice(font);
            let start = start as usize;
            let tables = u16::from_be_bytes([font[4], font[5]]) as usize;
            for table in 0..tables {
                let at = start + 12 + table * 16 + 8;
                let offset = u32::from_be_bytes(data[at..at + 4].try_into().unwrap());
                data[at..at + 4].copy_from_slice(&(offset + start as u32).to_be_bytes());
            }
        }
        Arc::new(data)
    }

    /// `font` with the family name `family`, written over the space of its own.
    fn renamed(font: &[u8], family: &str) -> Vec<u8> {
        let mut font = font.to_vec();
        let read = |font: &[u8], at: usize| u16::from_be_bytes([font[at], font[at + 1]]) as usize;
        let name = font[12..12 + read(&font, 4) * 16]
            .chunks(16)
            .find(|table| &table[..4] == b"name")
            .unwrap();
        let table = u32::from_be_bytes(name[8..12].try_into().unwrap()) as usize;
        let strings = table + read(&font, table + 4);
        let encoded: Vec<u8> = family.encode_utf16().flat_map(u16::to_be_bytes).collect();
        for record in 0..read(&font, table + 2) {
            let at = table + 6 + record * 12;
            if read(&font, at) == 3 && matches!(read(&font, at + 6), 1 | 16) {
                let start = strings + read(&font, at + 10);
                font[start..start + encoded.len()].copy_from_slice(&encoded);
                font[at + 8..at + 10].copy_from_slice(&(encoded.len() as u16).to_be_bytes());
            }
        }
        font
    }

    fn in_family(content: &str, family: &str, weight: u16) -> TextLayoutRequest {
        let mut request = TextLayoutRequest::new(content);
        request.spans[0].font_family = family.to_string();
        request.spans[0].font_weight = weight;
        request
    }

    #[test]
    fn every_face_of_a_collection_stays_for_glyph_fallback() {
        let mut system = TextSystem::new();
        let bundled = system.measure(&in_family("Ω", "Roboto", 400)).width;
        let latin_roboto = renamed(INTER, "Roboto");
        system
            .load_family("Roboto", "latin", vec![Arc::new(latin_roboto)])
            .unwrap();
        system
            .load_family(
                "Inter",
                "collection",
                vec![collection(&[INTER, &scaled_roboto(2)])],
            )
            .unwrap();

        let request = in_family("Ω", "Inter", 400);
        assert_ne!(
            system.shape(&request).place([0.0; 2]).glyphs[0]
                .key
                .0
                .glyph_id,
            0
        );
        assert!((system.measure(&request).width * 2.0 - bundled).abs() < 1.0);
    }

    #[test]
    fn styles_that_do_not_tie_do_not_depend_on_load_order() {
        let bundled = TextSystem::new()
            .measure(&in_family("WWW", "Roboto", 400))
            .width;
        let bold = with_weight(&scaled_roboto(2), 700);
        let load = |system: &mut TextSystem, first: bool| {
            if first {
                let fonts = vec![collection(&[INTER, &bold])];
                system.load_family("Inter", "collection", fonts).unwrap();
            } else {
                let fonts = vec![scaled_roboto(4)];
                system.load_family("Roboto", "regular", fonts).unwrap();
            }
        };
        for order in [[true, false], [false, true]] {
            let mut system = TextSystem::new();
            load(&mut system, order[0]);
            load(&mut system, order[1]);
            let bold = system.measure(&in_family("WWW", "Roboto", 700)).width;
            assert!((bold * 2.0 - bundled).abs() < 1.0, "{order:?} drew {bold}");
            let regular = system.measure(&in_family("WWW", "Roboto", 400)).width;
            assert!(
                (regular * 4.0 - bundled).abs() < 1.0,
                "{order:?} drew {regular}"
            );
        }
    }

    #[test]
    fn removing_a_family_keeps_what_others_brought() {
        let mut system = TextSystem::new();
        let request = in_family("xxx", "Inter", 400);
        system
            .load_family("Inter", "inter", vec![Arc::new(INTER.to_vec())])
            .unwrap();
        let full = system.measure(&request).width;
        let half_inter = scaled(INTER, 2);
        system
            .load_family(
                "Roboto",
                "collection",
                vec![collection(&[ROBOTO_REGULAR_BYTES, &half_inter])],
            )
            .unwrap();

        system.remove_family("Inter");
        assert!((system.measure(&request).width * 2.0 - full).abs() < 1.0);
        assert_eq!(system.family_resolver("Roboto"), Some("collection"));
    }

    #[test]
    fn text_in_a_family_no_face_answers_to_is_drawn_in_the_default_family() {
        let mut system = TextSystem::new();
        let missing = [400, 700].map(|weight| in_family("WWW", "Missing", weight));
        let bundled = missing
            .each_ref()
            .map(|request| system.measure(request).width);
        let inter = Arc::new(INTER.to_vec());
        system.load_family("Inter", "inter", vec![inter]).unwrap();
        let bold = with_weight(&scaled(INTER, 2), 700);
        system.load_family("Bold", "bold", vec![bold]).unwrap();
        assert_eq!(
            missing
                .each_ref()
                .map(|request| system.measure(request).width),
            bundled
        );

        // A family added later is still reached by name.
        assert_ne!(
            system.measure(&in_family("WWW", "Inter", 400)).width,
            bundled[0]
        );
    }

    #[test]
    fn the_latest_load_wins_a_tie_until_it_is_removed() {
        let mut system = TextSystem::new();
        let request = TextLayoutRequest::new("WWW");
        let bundled = system.measure(&request).width;
        let drawn_at = |system: &mut TextSystem| {
            let width = system.measure(&request).width;
            (bundled / width).round() as u16
        };

        system
            .load_family("Half", "half", vec![scaled_roboto(2)])
            .unwrap();
        assert_eq!(drawn_at(&mut system), 2);
        system
            .load_family("Quarter", "quarter", vec![scaled_roboto(4)])
            .unwrap();
        assert_eq!(drawn_at(&mut system), 4);
        system
            .load_family("Half", "half again", vec![scaled_roboto(2)])
            .unwrap();
        assert_eq!(drawn_at(&mut system), 2);
        system.remove_family("Half");
        assert_eq!(drawn_at(&mut system), 4);
        system.remove_family("Quarter");
        assert_eq!(drawn_at(&mut system), 1);
    }

    #[test]
    fn a_font_without_faces_fails_and_leaves_the_family_as_it_was() {
        let mut system = TextSystem::new();
        let request = TextLayoutRequest::new("WWW");
        let bundled = system.measure(&request).width;
        let generation = system.font_generation();

        assert_eq!(
            system.load_family(
                DEFAULT_FONT_FAMILY,
                "bad",
                vec![scaled_roboto(2), Arc::new(vec![1, 2, 3])]
            ),
            Err(NoFontFaces)
        );

        assert_eq!(system.family_resolver(DEFAULT_FONT_FAMILY), None);
        assert_eq!(system.font_generation(), generation);
        assert_eq!(system.font_system.db().len(), 1);
        assert_eq!(system.measure(&request).width, bundled);

        // A store with no data for the family has nothing to replace its faces with.
        system
            .load_family(DEFAULT_FONT_FAMILY, "empty", Vec::new())
            .unwrap();
        assert_eq!(system.family_resolver(DEFAULT_FONT_FAMILY), Some("empty"));
        assert_eq!(system.font_generation(), generation);
        assert_eq!(system.measure(&request).width, bundled);

        system
            .load_family(DEFAULT_FONT_FAMILY, "half", vec![scaled_roboto(2)])
            .unwrap();
        system
            .load_family(DEFAULT_FONT_FAMILY, "empty", Vec::new())
            .unwrap();
        assert_eq!(system.family_resolver(DEFAULT_FONT_FAMILY), Some("empty"));
        system
            .load_family("Inter", "inter", vec![Arc::new(INTER.to_vec())])
            .unwrap();
        assert!((system.measure(&request).width * 2.0 - bundled).abs() < 1.0);
    }

    #[test]
    fn loading_fonts_invalidates_measurements() {
        let mut system = TextSystem::new();
        let mut request = TextLayoutRequest::new("Variable weight");
        request.spans[0].font_family = "Inter".to_string();
        let fallback = system.measure(&request);
        let generation = system.font_generation();

        system.load_font_data(include_bytes!("../assets/inter/InterVariable-Latin.ttf").to_vec());

        assert_ne!(system.font_generation(), generation);
        assert!(system.shapes.is_empty());
        let loaded = system.measure(&request);
        assert_ne!(loaded.width, fallback.width);
        assert_eq!(loaded, system.shape(&request).measurement);
        assert_eq!(system.measure(&request), loaded);
    }
}
