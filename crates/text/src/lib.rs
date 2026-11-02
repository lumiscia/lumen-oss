//! Text layout and GPU-facing text data for Lumen.
//!
//! This crate deliberately owns text shaping, measurement, atlas bookkeeping,
//! and WGSL-facing buffer formats. Lumen nodes should depend on this layer
//! rather than coupling directly to a specific renderer such as glyphon.

use std::borrow::Cow;
use std::collections::{HashMap, hash_map::DefaultHasher};
use std::hash::{Hash, Hasher};
use std::ops::Range;

use cosmic_text::{
    Align, Attrs, AttrsList, Buffer, CacheKey, CacheKeyFlags, Family, FeatureTag, FontFeatures,
    FontSystem, LayoutGlyph, LineEnding, LineIter, Metrics, Shaping, Style, SwashCache,
    SwashContent, SwashImage, Weight, Wrap,
};
use etagere::{AllocId, size2};
use swash::scale::Source;

mod gpu;
#[cfg(feature = "msdf")]
mod msdf;
pub use gpu::*;

/// Text shader body; the engine prepends its alpha and paint WGSL chunks.
pub const TEXT_SHADER: &str = include_str!("shaders/text.wgsl");
pub const DEFAULT_FONT_FAMILY: &str = "Roboto";
pub const ROBOTO_REGULAR_BYTES: &[u8] = include_bytes!("../assets/roboto/Roboto-Regular.ttf");
/// Whether [`TextRenderMode::Msdf`] is compiled in.
pub const MSDF_SUPPORTED: bool = cfg!(feature = "msdf");
/// Em size, in atlas texels, of the coarsest distance fields.
pub const MSDF_EM_SIZE: f32 = 48.0;
/// Em size, in atlas texels, of the finest distance fields.
pub const MSDF_MAX_EM_SIZE: f32 = 384.0;
/// Drawn size at which a distance field with [`MSDF_EM_SIZE`] texels per em is
/// outgrown. Past it, fine corners such as serifs show as facets.
const MSDF_MAX_MAGNIFICATION: f32 = 4.0;
/// Font size from which [`TextRenderMode::Auto`] draws distance fields.
pub const MSDF_AUTO_MIN_SIZE: f32 = 512.0;
/// Width of the MSDF distance range in atlas texels, centred on the outline.
pub const MSDF_PX_RANGE: f32 = 6.0;
/// Most shapes a fit takes, counting the final one of text that fits at no factor tried.
const FIT_SHAPES: usize = 12;
/// Smallest factor a fit shrinks text by when its requested floor is not a positive number.
const MIN_FIT_SCALE: f32 = 0.01;
/// Most recent fits a text system keeps.
const FIT_CACHE: usize = 32;
/// Most recent measurements a text system keeps.
const MEASUREMENT_CACHE: usize = 32;
/// Pixels by which lines may pass a limit and still fit it.
const FIT_TOLERANCE: f32 = 0.01;
/// Stands in for the byte offset of an ellipsis' cluster, which is past every other.
const ELLIPSIS_CLUSTER: usize = usize::MAX;

/// How outline glyphs are drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum TextRenderMode {
    /// Glyphs are rasterized for their size and subpixel position.
    #[default]
    Raster,
    /// Outline glyphs share distance fields across sizes, regenerated at a
    /// finer resolution for each doubling of size past [`MSDF_EM_SIZE`] × 6.
    /// Colour, bitmap and overlapping-contour glyphs, and every glyph in builds
    /// without the `msdf` feature, are rasterized.
    Msdf,
    /// [`Self::Raster`] below [`MSDF_AUTO_MIN_SIZE`] and [`Self::Msdf`] from it.
    Auto,
}

/// Texels per em of the distance field drawn at `font_size`: the coarsest
/// that is not magnified more than [`MSDF_MAX_MAGNIFICATION`] times.
pub fn msdf_em_size(font_size: f32) -> f32 {
    let mut em = MSDF_EM_SIZE;
    while em < MSDF_MAX_EM_SIZE && font_size > em * MSDF_MAX_MAGNIFICATION {
        em *= 2.0;
    }
    em
}

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

#[derive(Debug)]
pub struct TextSystem {
    font_system: FontSystem,
    swash_cache: SwashCache,
    font_generation: u64,
    /// Recent fits as the request, without its origin and color, the factor it chose and how
    /// text wraps at it, newest last.
    fits: Vec<(TextLayoutRequest, f32, Wrap)>,
    /// Recent measurements keyed without origin and color, newest last.
    measurements: Vec<(TextLayoutRequest, TextMeasurement)>,
}

impl TextSystem {
    /// A text system with only the bundled fonts. Other fonts must be loaded with
    /// `load_font_data`, so text renders the same on every machine.
    pub fn new() -> Self {
        let mut font_system = FontSystem::new_with_locale_and_db_and_fallback(
            "en-US".to_string(),
            cosmic_text::fontdb::Database::new(),
            NoPlatformFallback,
        );
        load_default_fonts(&mut font_system);
        Self {
            font_system,
            swash_cache: SwashCache::new(),
            font_generation: 0,
            fits: Vec::new(),
            measurements: Vec::new(),
        }
    }

    pub fn load_font_data(&mut self, data: Vec<u8>) {
        self.font_system.db_mut().load_font_data(data);
        self.font_generation += 1;
        self.fits.clear();
        self.measurements.clear();
    }

    /// Changes whenever fonts are loaded, so shaped text can be invalidated.
    pub fn font_generation(&self) -> u64 {
        self.font_generation
    }

    pub fn measure(&mut self, request: &TextLayoutRequest) -> TextMeasurement {
        let key = request.cache_key();
        if let Some((_, measurement)) = self.measurements.iter().find(|(cached, _)| *cached == key)
        {
            return *measurement;
        }
        let measurement = self.shape(request).measurement;
        if self.measurements.len() == MEASUREMENT_CACHE {
            self.measurements.remove(0);
        }
        self.measurements.push((key, measurement));
        measurement
    }

    pub fn layout(&mut self, request: &TextLayoutRequest) -> TextLayout {
        self.shape(request).place(request.origin, request.color)
    }

    /// Shapes text without placing it, so it can be placed at many origins.
    pub fn shape(&mut self, request: &TextLayoutRequest) -> ShapedText {
        let Some(min_scale) = request.min_fit_scale.filter(|_| request.is_limited()) else {
            return self.shape_lines(request, Wrap::WordOrGlyph, true);
        };
        // Measurement, bounds and drawing each shape the same text, so they share its fit.
        let key = request.cache_key();
        let cached = self.fits.iter().find(|(fitted, ..)| *fitted == key);
        let (scale, wrap, fitted) = match cached {
            Some(&(_, scale, wrap)) => (scale, wrap, None),
            None => {
                let min_scale = if min_scale > 0.0 {
                    min_scale.min(1.0)
                } else {
                    MIN_FIT_SCALE
                };
                let (scale, fitted) = self.fit(request, min_scale);
                // Text that fits keeps its words whole, as the fit shaped them, even where it
                // overflows by less than the fit's tolerance.
                let wrap = if fitted.is_some() {
                    Wrap::Word
                } else {
                    Wrap::WordOrGlyph
                };
                if self.fits.len() == FIT_CACHE {
                    self.fits.remove(0);
                }
                self.fits.push((key, scale, wrap));
                (scale, wrap, fitted)
            }
        };
        // A fit can leave more paragraphs than `max_lines`, which only a limited shape drops.
        let mut shaped = fitted
            .filter(|shaped| {
                request
                    .max_lines
                    .is_none_or(|max| shaped.counts.line <= max)
            })
            .unwrap_or_else(|| self.shape_lines(&request.scaled(scale), wrap, true));
        shaped.fit_scale = scale;
        shaped
    }

    /// The largest factor from `min_scale` to 1 that makes `request` fit its limits when every
    /// size scales by it, without breaking words, and the text shaped at it. Without one that
    /// fits within one shape less than [`FIT_SHAPES`], which leaves one to shape the text at
    /// it, `min_scale` and no text.
    fn fit(&mut self, request: &TextLayoutRequest, min_scale: f32) -> (f32, Option<ShapedText>) {
        let mut best: Option<(f32, ShapedText)> = None;
        // The smallest factor known not to fit, or 1, past which text never grows.
        let mut overflowing: f32 = 1.0;
        let mut scale = 1.0;
        for _ in 1..FIT_SHAPES {
            let shaped = self.shape_lines(&request.scaled(scale), Wrap::Word, false);
            let ratio = request.fit_ratio(&shaped);
            if ratio >= 1.0 {
                best = Some((scale, shaped));
            } else {
                overflowing = scale;
            }
            scale = match &best {
                Some((fits, _)) if overflowing / fits > 1.002 => (fits * overflowing).sqrt(),
                Some(_) => break,
                None if scale <= min_scale => break,
                None => (scale * ratio.min(0.98)).max(min_scale),
            };
        }
        best.map_or((min_scale, None), |(scale, shaped)| (scale, Some(shaped)))
    }

    fn shape_lines(&mut self, request: &TextLayoutRequest, wrap: Wrap, limit: bool) -> ShapedText {
        let fallback = [TextSpan::new("")];
        let spans = if request.spans.is_empty() {
            &fallback[..]
        } else {
            &request.spans[..]
        };
        // cosmic-text 0.19 merges adjacent spans with the same family, weight, style and
        // stretch into one shaping run, so a ligature or grapheme cluster that crosses a span
        // boundary takes the first span's size and spacing, and the later span gets no glyphs
        // from that cluster. Features are not compared either, so a run takes those of its
        // first span.
        let attrs: Vec<Attrs> = spans
            .iter()
            .enumerate()
            .map(|(index, span)| {
                let metrics = request.span_metrics(span);
                let mut attrs = Attrs::new()
                    .family(Family::Name(&span.font_family))
                    .weight(Weight(span.font_weight.clamp(1, 1000)))
                    .style(match span.font_style {
                        TextFontStyle::Italic => Style::Italic,
                        TextFontStyle::Oblique => Style::Oblique,
                        TextFontStyle::Normal => Style::Normal,
                    })
                    .cache_key_flags(if request.disable_hinting {
                        CacheKeyFlags::DISABLE_HINTING
                    } else {
                        CacheKeyFlags::empty()
                    })
                    // Each line is as tall as its tallest glyph's line height.
                    .metrics(metrics)
                    .metadata(index);
                if !span.font_features.is_empty() {
                    let mut features = FontFeatures::new();
                    for feature in &span.font_features {
                        features.set(FeatureTag::new(&feature.tag), feature.value);
                    }
                    attrs = attrs.font_features(features);
                }
                if span.letter_spacing != 0.0 {
                    // cosmic-text takes tracking in em and adds it to every glyph advance.
                    attrs = attrs.letter_spacing(span.letter_spacing / metrics.font_size);
                }
                attrs
            })
            .collect();
        let mut buffer = Buffer::new(&mut self.font_system, request.span_metrics(&spans[0]));
        let wraps = request.max_width.is_some_and(|width| width > 0.0);
        // cosmic-text adds tracking after every glyph, including the last on a
        // line, so every line is its last glyph's `letter_spacing` wider than its
        // ink. Widening the layout width by the narrowest spacing makes wrapping
        // see trimmed line widths; lines ending in a wider spacing wrap early by
        // the difference. cosmic-text clamps negative widths to zero, so a
        // negative spacing larger than `max_width` wraps less than it should.
        let widening = spans
            .iter()
            .map(|span| span.letter_spacing)
            .fold(f32::INFINITY, f32::min);
        let layout_width = request
            .max_width
            .filter(|width| *width > 0.0)
            .map(|width| width + widening);
        buffer.set_size(layout_width, None);
        buffer.set_wrap(if wraps { wrap } else { Wrap::None });
        let transformed = spans
            .iter()
            .any(|span| span.transform != TextTransform::None)
            .then(|| transformed_texts(spans));
        let span_text = |index: usize| match &transformed {
            Some(texts) => texts[index].as_ref(),
            None => spans[index].text.as_str(),
        };
        let text: String = (0..spans.len()).map(span_text).collect();
        let span_ranges: Vec<Range<usize>> = (0..spans.len())
            .scan(0, |end, index| {
                let start = *end;
                *end += span_text(index).len();
                Some(start..*end)
            })
            .collect();
        // Paragraphs split at CR and LF, with a final empty one after a trailing line ending.
        let mut paragraphs: Vec<Range<usize>> = Vec::new();
        let mut last_ending = LineEnding::default();
        for (range, ending) in LineIter::new(&text) {
            paragraphs.push(range);
            last_ending = ending;
        }
        if last_ending != LineEnding::None {
            paragraphs.push(text.len()..text.len());
        }
        buffer.set_text(
            &text,
            &attrs[0],
            Shaping::Advanced,
            Some(match request.align {
                TextAlign::Left => Align::Left,
                TextAlign::Center => Align::Center,
                TextAlign::Right => Align::Right,
                TextAlign::Justified => Align::Justified,
            }),
        );
        for (line, paragraph) in buffer.lines.iter_mut().zip(&paragraphs) {
            // An empty line is as tall as the span that owns its line ending, or else its
            // preceding one.
            let owner_byte = if paragraph.is_empty() && paragraph.end == text.len() {
                paragraph.start.saturating_sub(1)
            } else {
                paragraph.start
            };
            let owner = span_ranges
                .iter()
                .position(|range| range.contains(&owner_byte))
                .unwrap_or(0);
            let mut list = AttrsList::new(&attrs[owner]);
            for (range, attrs) in span_ranges.iter().zip(&attrs) {
                let start = range.start.max(paragraph.start);
                let end = range.end.min(paragraph.end);
                if start < end && *attrs != list.defaults() {
                    list.add_span(start - paragraph.start..end - paragraph.start, attrs);
                }
            }
            line.set_attrs_list(list);
        }
        buffer.shape_until_scroll(&mut self.font_system, true);

        let spacing = |glyph: &LayoutGlyph| {
            spans
                .get(glyph.metadata)
                .map_or(0.0, |span| span.letter_spacing)
        };
        let lift = |span: usize| spans.get(span).map_or(0.0, |span| span.baseline_shift);
        let align_factor = match request.align {
            TextAlign::Center => 0.5,
            TextAlign::Right => 1.0,
            TextAlign::Left | TextAlign::Justified => 0.0,
        };

        let mut fonts = HashMap::new();
        let mut span_metrics: Vec<Option<TextSpanMetrics>> = vec![None; spans.len()];
        let mut glyphs = Vec::new();
        let mut segments = Vec::new();
        // Each glyph's cluster as `(paragraph, byte offset)`, which orders clusters logically.
        let mut clusters = Vec::new();
        let mut words = WordCounter::default();
        let mut lines = 0;
        let mut width: f32 = 0.0;
        let mut height: f32 = 0.0;
        let mut x_min = f32::INFINITY;
        let mut x_max = f32::NEG_INFINITY;
        // The vertical extent of the line boxes, which spans with a baseline shift move.
        let mut y_min: f32 = 0.0;
        let mut y_max: f32 = 0.0;
        let runs: Vec<_> = buffer.layout_runs().collect();
        let max_width = request.max_width.filter(|_| wraps);
        let shown = if limit && (request.max_lines.is_some() || request.max_height.is_some()) {
            runs.iter()
                .take(request.max_lines.map_or(usize::MAX, |lines| lines as usize))
                .take_while(|run| {
                    request.max_height.is_none_or(|height| {
                        let paragraph_offset = request.paragraph_spacing * run.line_i as f32;
                        run.line_top + paragraph_offset + run.line_height <= height + FIT_TOLERANCE
                    })
                })
                .count()
        } else {
            runs.len()
        };
        let truncated = shown < runs.len();
        let mut has_ellipsis = false;
        for (run_index, run) in runs[..shown].iter().enumerate() {
            let mut line_glyphs = run.glyphs.to_vec();
            // cosmic-text aligned the untrimmed line within the widened width. Justified
            // lines are not shifted.
            let aligned = layout_width.map_or(0.0, |layout_width| {
                align_factor * (layout_width - run.line_w).max(0.0)
            });
            let mut line_w = run.line_w;
            // The ellipsis glyphs' range in `line_glyphs`.
            let mut ellipsis = 0..0;
            if truncated && request.overflow == TextOverflow::Ellipsis && run_index + 1 == shown {
                let fallback_span = glyphs
                    .last()
                    .map_or(0, |shaped: &ShapedGlyph| shaped.item.span);
                (line_w, ellipsis) = self.ellipsize(
                    &mut line_glyphs,
                    run,
                    spans,
                    &attrs,
                    request,
                    max_width,
                    aligned,
                    fallback_span as usize,
                );
                has_ellipsis = true;
            }
            // The line's trailing tracking is that of the glyph whose advance ends it on the
            // right: the rightmost of a line closed up around an ellipsis, or else the one
            // cosmic-text placed first in a right-to-left line and last in a left-to-right one.
            let trailing_index = if !ellipsis.is_empty() {
                let end = |index: usize| line_glyphs[index].x + line_glyphs[index].w;
                (0..line_glyphs.len()).max_by(|&a, &b| end(a).total_cmp(&end(b)))
            } else if run.rtl {
                (!line_glyphs.is_empty()).then_some(0)
            } else {
                line_glyphs.len().checked_sub(1)
            };
            let trailing = trailing_index.map_or(0.0, |index| spacing(&line_glyphs[index]));
            let mut ink_width = line_w - trailing;
            let ends_paragraph = runs
                .get(run_index + 1)
                .is_none_or(|next| next.line_i != run.line_i);
            if let Some(max_width) = max_width
                && request.align == TextAlign::Justified
                && !ends_paragraph
                && ellipsis.is_empty()
            {
                // cosmic-text stretched the line to the widened width, which leaves it short
                // by the difference between its trailing tracking and the narrowest. That
                // is spread over the same spaces.
                let is_space = |glyph: &LayoutGlyph| {
                    let cluster = &run.text[glyph.start..glyph.end];
                    !cluster.is_empty() && cluster.chars().all(char::is_whitespace)
                };
                let spaces = line_glyphs.iter().filter(|glyph| is_space(glyph)).count();
                let gap = max_width - ink_width;
                if spaces > 0 && gap > 0.0 {
                    ink_width = max_width;
                    let extra = gap / spaces as f32;
                    let mut order: Vec<usize> = (0..line_glyphs.len()).collect();
                    order.sort_by(|&a, &b| line_glyphs[a].x.total_cmp(&line_glyphs[b].x));
                    let mut moved = 0.0;
                    for index in order {
                        let glyph = &mut line_glyphs[index];
                        glyph.x += moved;
                        if is_space(glyph) {
                            glyph.w += extra;
                            moved += extra;
                        }
                    }
                }
            }
            let shift = match max_width {
                Some(max_width) => align_factor * (max_width - ink_width).max(0.0) - aligned,
                None => 0.0,
            };
            if !line_glyphs.is_empty() {
                width = width.max(ink_width);
            }
            // Paragraph spacing pushes each paragraph down by that of the ones before it.
            let paragraph_offset = request.paragraph_spacing * run.line_i as f32;
            let line_y = run.line_y + paragraph_offset;
            let line_top = run.line_top + paragraph_offset;
            height = height.max(line_top + run.line_height);
            words.enter(run.line_i, run.text);
            let line_middle = line_top + run.line_height * 0.5;
            let paragraph_start = paragraphs[run.line_i].start;
            let first = glyphs.len();
            for (index, mut glyph) in line_glyphs.into_iter().enumerate() {
                glyph.x += shift;
                let span = glyph.metadata.min(spans.len() - 1);
                x_min = x_min.min(glyph.x);
                x_max = x_max.max(glyph.x + glyph.w - spacing(&glyph));
                clusters.push((
                    run.line_i,
                    if ellipsis.contains(&index) {
                        ELLIPSIS_CLUSTER
                    } else {
                        glyph.start
                    },
                ));
                if span_metrics[span].is_none() {
                    let font = self.font_metrics(&mut fonts, &glyph);
                    span_metrics[span] = Some(font.decorations(spans[span].font_size.max(1.0)));
                }
                let lift = lift(span);
                glyphs.push(ShapedGlyph {
                    line_y: line_y - lift,
                    item: TextItem {
                        glyph: 0,
                        word: words.word_at(&run.text[glyph.start..glyph.end], glyph.start),
                        line: lines,
                        span: span as u32,
                    },
                    pivot: [0.0, line_middle - lift],
                    glyph,
                });
            }
            // A cluster turns about the middle of its advance, which its marks share.
            for cluster in glyphs[first..].chunk_by_mut(|a, b| a.glyph.start == b.glyph.start) {
                let (min, max) = cluster.iter().fold(
                    (f32::INFINITY, f32::NEG_INFINITY),
                    |(min, max), shaped| {
                        let glyph = &shaped.glyph;
                        (
                            min.min(glyph.x),
                            max.max(glyph.x + glyph.w - spacing(glyph)),
                        )
                    },
                );
                let middle = (min + max.max(min)) * 0.5;
                for shaped in cluster {
                    shaped.pivot[0] = middle;
                }
            }
            // Segments follow the line from left to right, splitting where the span or direction
            // changes, and around an ellipsis, which covers no bytes.
            let mut order: Vec<usize> = (first..glyphs.len()).collect();
            order.sort_by(|&a, &b| glyphs[a].glyph.x.total_cmp(&glyphs[b].glyph.x));
            for run_glyphs in order.chunk_by(|&a, &b| {
                let ellipses = [a, b].map(|index| ellipsis.contains(&(index - first)));
                let (a, b) = (&glyphs[a], &glyphs[b]);
                a.item.span == b.item.span
                    && a.glyph.level.is_rtl() == b.glyph.level.is_rtl()
                    && ellipses[0] == ellipses[1]
            }) {
                let lead = &glyphs[run_glyphs[0]];
                let span_range = &span_ranges[lead.item.span as usize];
                let shift = lift(lead.item.span as usize);
                let bytes = |glyph: &LayoutGlyph| {
                    let clamp = |byte: usize| {
                        (paragraph_start + byte).clamp(span_range.start, span_range.end)
                            - span_range.start
                    };
                    clamp(glyph.start)..clamp(glyph.end)
                };
                let mut segment = TextSegment {
                    span: lead.item.span,
                    line: lines,
                    x: [f32::INFINITY, f32::NEG_INFINITY],
                    baseline: line_y - shift,
                    shift,
                    line_top,
                    line_height: run.line_height,
                    ascent: 0.0,
                    descent: 0.0,
                    bytes: bytes(&lead.glyph),
                    rtl: lead.glyph.level.is_rtl(),
                };
                for &index in run_glyphs {
                    let glyph = &glyphs[index].glyph;
                    let font = self.font_metrics(&mut fonts, glyph);
                    let mut end = glyph.x + glyph.w;
                    if Some(index - first) == trailing_index {
                        end -= trailing;
                    }
                    segment.x = [segment.x[0].min(glyph.x), segment.x[1].max(end)];
                    segment.ascent = segment.ascent.max(font.ascent * glyph.font_size);
                    segment.descent = segment.descent.max(font.descent * glyph.font_size);
                    let glyph_bytes = bytes(glyph);
                    segment.bytes = segment.bytes.start.min(glyph_bytes.start)
                        ..segment.bytes.end.max(glyph_bytes.end);
                }
                segment.x[1] = segment.x[1].max(segment.x[0]);
                y_min = y_min.min(line_top - shift);
                y_max = y_max.max(line_top + run.line_height - shift);
                segments.push(segment);
            }
            lines += 1;
        }
        // Wrapping drops the whitespace it breaks at from the runs, so clusters are taken from
        // the unwrapped shaping too, which keeps numbering independent of the wrap width.
        let mut ordered = clusters.clone();
        // Truncated text shows a logical prefix of its clusters, which keep their numbers, and
        // the ellipsis is numbered after them.
        let last_shown = truncated.then(|| {
            ordered.retain(|cluster| cluster.1 != ELLIPSIS_CLUSTER);
            ordered.iter().max().copied()
        });
        for (paragraph, line) in buffer.lines.iter().enumerate() {
            let shaping = line.shape_opt().into_iter().flat_map(|shape| &shape.spans);
            ordered.extend(
                shaping
                    .flat_map(|span| &span.words)
                    .flat_map(|word| &word.glyphs)
                    .map(|glyph| (paragraph, glyph.start)),
            );
        }
        ordered.sort_unstable();
        ordered.dedup();
        if let Some(last_shown) = last_shown {
            ordered.retain(|cluster| last_shown.is_some_and(|last| *cluster <= last));
        }
        for (shaped, cluster) in glyphs.iter_mut().zip(&clusters) {
            shaped.item.glyph = ordered.binary_search(cluster).unwrap_or_else(|index| index) as u32;
        }
        let y_max = y_max.max(height);
        let bounds = if glyphs.is_empty() {
            [0.0, y_min, 0.0, y_max - y_min]
        } else {
            [x_min, y_min, x_max - x_min, y_max - y_min]
        };

        let mut span_boxes: Vec<Option<[f32; 4]>> = vec![None; request.spans.len()];
        for segment in &segments {
            let top = segment.line_top - segment.shift;
            let edges = [segment.x[0], top, segment.x[1], top + segment.line_height];
            if let Some(span) = span_boxes.get_mut(segment.span as usize) {
                *span = Some(span.map_or(edges, |a| {
                    [
                        a[0].min(edges[0]),
                        a[1].min(edges[1]),
                        a[2].max(edges[2]),
                        a[3].max(edges[3]),
                    ]
                }));
            }
        }
        for [x0, y0, x1, y1] in span_boxes.iter_mut().flatten() {
            (*x1, *y1) = (*x1 - *x0, *y1 - *y0);
        }

        ShapedText {
            measurement: TextMeasurement { width, height },
            bounds,
            counts: TextItem {
                glyph: ordered.len() as u32 + u32::from(has_ellipsis),
                word: if truncated {
                    glyphs
                        .iter()
                        .map(|shaped| shaped.item.word + 1)
                        .max()
                        .unwrap_or(0)
                } else {
                    words.total()
                },
                line: lines,
                span: request.spans.len() as u32,
            },
            segments,
            span_boxes,
            span_metrics: span_metrics
                .into_iter()
                .take(request.spans.len())
                .map(Option::unwrap_or_default)
                .collect(),
            span_starts: span_ranges
                .iter()
                .map(|range| range.start)
                .chain([text.len()])
                .collect(),
            fit_scale: 1.0,
            paragraphs: paragraphs.len() as u32,
            glyphs,
        }
    }

    /// Ends `line`, the glyphs of `run` from left to right, with an ellipsis in reading order:
    /// on the right of a left-to-right paragraph and the left of a right-to-left one. Its
    /// logically last clusters, and whitespace before the ellipsis, are dropped until the line
    /// fits `max_width`, and the rest close up. The ellipsis takes the attrs of the span of the
    /// last glyph kept, else of the line's first, else `fallback_span`'s. A line without glyphs
    /// starts at `aligned`. Returns the line's width and the ellipsis glyphs' range in `line`.
    #[allow(
        clippy::too_many_arguments,
        reason = "the ellipsis reads the line's shaping state from `shape_lines`"
    )]
    fn ellipsize(
        &mut self,
        line: &mut Vec<LayoutGlyph>,
        run: &cosmic_text::LayoutRun,
        spans: &[TextSpan],
        attrs: &[Attrs],
        request: &TextLayoutRequest,
        max_width: Option<f32>,
        aligned: f32,
        fallback_span: usize,
    ) -> (f32, Range<usize>) {
        let spacing = |glyph: &LayoutGlyph| {
            spans
                .get(glyph.metadata)
                .map_or(0.0, |span| span.letter_spacing)
        };
        let left = line
            .iter()
            .map(|glyph| glyph.x)
            .fold(f32::INFINITY, f32::min);
        let mut order: Vec<usize> = (0..line.len()).collect();
        order.sort_by_key(|&index| (line[index].start, index));
        let fallback_span = order
            .first()
            .map_or(fallback_span, |&first| line[first].metadata);
        let mut ellipses: HashMap<usize, Vec<LayoutGlyph>> = HashMap::new();
        // Glyphs `order[..kept]` are kept.
        let mut kept = order.len();
        let mut kept_width: f32 = line.iter().map(|glyph| glyph.w).sum();
        let mut ellipsis = loop {
            let last = kept.checked_sub(1).map(|last| &line[order[last]]);
            let span = last
                .map_or(fallback_span, |glyph| glyph.metadata)
                .min(spans.len() - 1);
            let ellipsis = ellipses.entry(span).or_insert_with(|| {
                let mut buffer =
                    Buffer::new(&mut self.font_system, request.span_metrics(&spans[span]));
                buffer.set_text("\u{2026}", &attrs[span], Shaping::Advanced, None);
                buffer.shape_until_scroll(&mut self.font_system, true);
                buffer
                    .layout_runs()
                    .flat_map(|run| run.glyphs.iter().cloned())
                    .collect()
            });
            let Some(last) = last else {
                break ellipsis.clone();
            };
            // The line ends on the right in the ellipsis, or in the rightmost glyph kept when
            // the ellipsis is on its left, without that glyph's tracking.
            let rightmost = if run.rtl {
                order[..kept]
                    .iter()
                    .map(|&index| &line[index])
                    .max_by(|a, b| (a.x + a.w).total_cmp(&(b.x + b.w)))
            } else {
                ellipsis.last()
            };
            let width: f32 =
                ellipsis.iter().map(|glyph| glyph.w).sum::<f32>() - rightmost.map_or(0.0, spacing);
            let space = run.text[last.start..last.end]
                .chars()
                .all(char::is_whitespace);
            if !space && max_width.is_none_or(|max| kept_width + width <= max + FIT_TOLERANCE) {
                break ellipsis.clone();
            }
            let start = last.start;
            while kept > 0 && line[order[kept - 1]].start == start {
                kept -= 1;
                kept_width -= line[order[kept]].w;
            }
        };
        let cut = match kept.checked_sub(1) {
            Some(last) => line[order[last]].end,
            None => order.first().map_or(0, |&first| line[first].start),
        };
        let mut removed = vec![false; line.len()];
        for &index in &order[kept..] {
            removed[index] = true;
        }
        let removed_glyphs: Vec<&LayoutGlyph> = (0..line.len())
            .filter(|&index| removed[index])
            .map(|index| &line[index])
            .collect();
        let mut kept_glyphs: Vec<LayoutGlyph> = (0..line.len())
            .filter(|&index| !removed[index])
            .map(|index| {
                let mut glyph = line[index].clone();
                glyph.x -= removed_glyphs
                    .iter()
                    .filter(|removed| removed.x < glyph.x)
                    .map(|removed| removed.w)
                    .sum::<f32>();
                glyph
            })
            .collect();
        let left = if left.is_finite() { left } else { aligned };
        let ellipsis_width: f32 = ellipsis.iter().map(|glyph| glyph.w).sum();
        for glyph in &mut ellipsis {
            (glyph.start, glyph.end) = (cut, cut);
            glyph.level = if run.rtl {
                unicode_bidi::Level::rtl()
            } else {
                unicode_bidi::Level::ltr()
            };
            glyph.x += if run.rtl { left } else { left + kept_width };
        }
        let range = if run.rtl {
            for glyph in &mut kept_glyphs {
                glyph.x += ellipsis_width;
            }
            let range = 0..ellipsis.len();
            ellipsis.append(&mut kept_glyphs);
            *line = ellipsis;
            range
        } else {
            let range = kept_glyphs.len()..kept_glyphs.len() + ellipsis.len();
            kept_glyphs.append(&mut ellipsis);
            *line = kept_glyphs;
            range
        };
        (kept_width + ellipsis_width, range)
    }

    /// Line and decoration metrics of `glyph`'s font, cached in `fonts`.
    fn font_metrics(
        &mut self,
        fonts: &mut HashMap<(cosmic_text::fontdb::ID, Weight), FontMetrics>,
        glyph: &LayoutGlyph,
    ) -> FontMetrics {
        *fonts
            .entry((glyph.font_id, glyph.font_weight))
            .or_insert_with(|| {
                self.font_system
                    .get_font(glyph.font_id, glyph.font_weight)
                    .map_or_else(FontMetrics::default, |font| FontMetrics::new(&font))
            })
    }

    /// The pixels `shaped` may draw to, as `[min_x, min_y, max_x, max_y]` from its origin, in any
    /// render mode. This is the union of its glyph outlines grown by a margin for antialiasing,
    /// subpixel snapping and distance field padding. `None` when a glyph's extent is unknown.
    pub fn ink_bounds(&mut self, shaped: &ShapedText) -> Option<[f32; 4]> {
        let mut outlines: HashMap<CacheKey, Option<[f32; 4]>> = HashMap::new();
        let mut ink: Option<[f32; 4]> = None;
        for shaped_glyph in &shaped.glyphs {
            let glyph = &shaped_glyph.glyph;
            let position = [
                glyph.x + glyph.font_size * glyph.x_offset,
                glyph.y - glyph.font_size * glyph.y_offset + shaped_glyph.line_y,
            ];
            let (key, ..) = CacheKey::new(
                glyph.font_id,
                glyph.glyph_id,
                glyph.font_size,
                (0.0, 0.0),
                glyph.font_weight,
                glyph.cache_key_flags,
            );
            let outline = match outlines.get(&key) {
                Some(outline) => *outline,
                None => {
                    let outline = self.glyph_extent(key)?;
                    outlines.insert(key, outline);
                    outline
                }
            };
            let Some([min_x, min_y, max_x, max_y]) = outline else {
                continue;
            };
            let margin = 2.0 + 0.1 * glyph.font_size;
            let glyph_ink = [
                position[0] + min_x - margin,
                position[1] - max_y - margin,
                position[0] + max_x + margin,
                position[1] - min_y + margin,
            ];
            ink = Some(match ink {
                Some(ink) => [
                    ink[0].min(glyph_ink[0]),
                    ink[1].min(glyph_ink[1]),
                    ink[2].max(glyph_ink[2]),
                    ink[3].max(glyph_ink[3]),
                ],
                None => glyph_ink,
            });
        }
        let bounds = ink.unwrap_or([0.0; 4]);
        bounds
            .iter()
            .all(|value| value.is_finite())
            .then_some(bounds)
    }

    /// A glyph's `[min_x, min_y, max_x, max_y]` with Y pointing up from its pen position, or
    /// `Some(None)` when it draws nothing. `None` when its extent cannot be determined.
    fn glyph_extent(&mut self, key: CacheKey) -> Option<Option<[f32; 4]>> {
        if self.is_plain_outline(key)
            && let Some(commands) = self
                .swash_cache
                .get_outline_commands_uncached(&mut self.font_system, key)
        {
            return Some(outline_bounds(&commands));
        }
        let Some(image) = self
            .swash_cache
            .get_image_uncached(&mut self.font_system, key)
        else {
            return Some(None);
        };
        let placement = image.placement;
        Some((placement.width > 0 && placement.height > 0).then_some([
            placement.left as f32,
            placement.top as f32 - placement.height as f32,
            placement.left as f32 + placement.width as f32,
            placement.top as f32,
        ]))
    }

    /// Whether the glyph's font draws it from its outline alone, with no colour or bitmap tables.
    fn is_plain_outline(&mut self, key: CacheKey) -> bool {
        self.font_system
            .get_font(key.font_id, key.font_weight)
            .is_some_and(|font| {
                ![b"COLR", b"CBDT", b"sbix", b"EBDT"]
                    .iter()
                    .any(|tag| font.as_swash().table(u32::from_be_bytes(**tag)).is_some())
            })
    }

    /// Draws `layout` into a fresh `size`² atlas and returns its pixels.
    #[cfg(test)]
    fn render_atlas(
        &mut self,
        layout: &TextLayout,
        mode: TextRenderMode,
        size: u32,
    ) -> AtlasRender {
        let mut atlas = GlyphAtlas::new(size);
        atlas.begin_frame();
        let prepared = self.prepare_glyphs(&mut atlas, layout, mode, &mut None);
        let mut pixels = vec![0_u8; size as usize * size as usize * 4];
        for upload in &prepared.uploads {
            let row_len = upload.size[0] as usize * 4;
            for row in 0..upload.size[1] as usize {
                let dst = ((upload.origin[1] as usize + row) * size as usize
                    + upload.origin[0] as usize)
                    * 4;
                pixels[dst..dst + row_len]
                    .copy_from_slice(&upload.pixels[row * row_len..(row + 1) * row_len]);
            }
        }
        AtlasRender {
            pixels,
            instances: prepared.instances,
        }
    }

    /// Places `layout` in `atlas` for its current frame, generating glyphs that
    /// are not resident yet one at a time.
    ///
    /// When the atlas is full, the least recently used glyphs that no layout
    /// used this frame are evicted. If there is still no room and only this
    /// layout uses the atlas this frame, the atlas is cleared and repacked.
    /// Glyphs that still do not fit are dropped.
    /// `overfull` suppresses retries until the working set or reclaimable space
    /// changes. Uploads become resident after [`GlyphAtlas::commit`].
    pub fn prepare_glyphs(
        &mut self,
        atlas: &mut GlyphAtlas,
        layout: &TextLayout,
        mode: TextRenderMode,
        overfull: &mut Option<u64>,
    ) -> PreparedGlyphs {
        self.prepare_stroked_glyphs(atlas, layout, mode, &[], overfull)
    }

    /// [`Self::prepare_glyphs`], also stroking the outline glyphs of each span `strokes` gives
    /// a positive half width, by that many pixels to each side. A stroke follows the path its
    /// glyph takes: a true distance field where the glyph draws one, else the stroked raster
    /// outline, or a true distance field where that is too large for the atlas. A field's range
    /// doubles as often as the stroke needs, and a stroke whose field is too large for the
    /// atlas is dropped.
    pub fn prepare_stroked_glyphs(
        &mut self,
        atlas: &mut GlyphAtlas,
        layout: &TextLayout,
        mode: TextRenderMode,
        strokes: &[f32],
        overfull: &mut Option<u64>,
    ) -> PreparedGlyphs {
        let glyphs = &layout.glyphs;
        let overflow_key = |atlas: &mut GlyphAtlas| {
            let mut keys = atlas.keys_for(glyphs, mode, strokes);
            keys.sort_unstable();
            let mut hasher = DefaultHasher::new();
            (keys, atlas.availability).hash(&mut hasher);
            hasher.finish()
        };
        let retry_new = overfull.is_none_or(|key| key != overflow_key(atlas));
        let mut uploads = Vec::new();
        let compacted = !self.place_glyphs(atlas, glyphs, mode, strokes, retry_new, &mut uploads)
            && atlas.layouts_this_frame == 0;
        if compacted {
            atlas.clear();
            uploads.clear();
            self.place_glyphs(atlas, glyphs, mode, strokes, true, &mut uploads);
        }

        let mut instances = Vec::with_capacity(glyphs.len());
        let mut sources = Vec::with_capacity(glyphs.len());
        let mut stroke_instances = Vec::new();
        let mut stroke_glyphs = Vec::new();
        let mut capacity_dropped = 0;
        for (index, glyph) in glyphs.iter().enumerate() {
            let key = atlas.key_for(glyph.key, mode);
            match atlas.glyphs.get(&key) {
                Some(AtlasGlyph {
                    image: AtlasImage::Placed(placed),
                    ..
                }) => {
                    instances.push(placed.instance(glyph, key));
                    sources.push(index);
                    let Some(half_width) = glyph_stroke(strokes, glyph) else {
                        continue;
                    };
                    let key = atlas.stroke_key_for(glyph.key, mode, half_width);
                    match atlas.glyphs.get(&key).map(|stroke| stroke.image) {
                        Some(AtlasImage::Placed(stroke)) => {
                            stroke_instances.push(stroke.instance(glyph, key));
                            stroke_glyphs.push(instances.len() - 1);
                        }
                        Some(AtlasImage::Empty) => {}
                        _ => capacity_dropped += 1,
                    }
                }
                Some(AtlasGlyph {
                    image: AtlasImage::Empty,
                    ..
                }) => {}
                _ => {
                    // The stroke is omitted along with its glyph.
                    let stroked = glyph_stroke(strokes, glyph).is_some()
                        && self.is_plain_outline(glyph.key.0);
                    capacity_dropped += 1 + usize::from(stroked);
                }
            }
        }
        *overfull = (capacity_dropped > 0).then(|| overflow_key(atlas));

        atlas.layouts_this_frame += 1;
        PreparedGlyphs {
            frame: atlas.frame,
            instances,
            sources,
            strokes: stroke_instances,
            stroke_glyphs,
            uploads,
            dropped_glyphs: capacity_dropped,
            compacted,
        }
    }

    /// Returns `false` when the atlas ran out of room.
    fn place_glyphs(
        &mut self,
        atlas: &mut GlyphAtlas,
        glyphs: &[TextGlyph],
        mode: TextRenderMode,
        strokes: &[f32],
        retry_new: bool,
        uploads: &mut Vec<GlyphUpload>,
    ) -> bool {
        // A second pass places raster glyphs for distance fields that turned
        // out to be raster-only.
        loop {
            let keys = atlas.keys_for(glyphs, mode, strokes);
            atlas.touch(&keys);
            let markers = atlas.markers;
            if !self.place_missing(atlas, &keys, retry_new, uploads) {
                return false;
            }
            if atlas.markers == markers || atlas.keys_for(glyphs, mode, strokes) == keys {
                return true;
            }
        }
    }

    /// Renders and places the glyphs of `keys` that are not resident, writing
    /// each one before rendering the next. Returns `false` when the atlas has no
    /// room for the next glyph. Glyphs not already cached are skipped unless
    /// `retry_new`.
    fn place_missing(
        &mut self,
        atlas: &mut GlyphAtlas,
        keys: &[AtlasKey],
        retry_new: bool,
        uploads: &mut Vec<GlyphUpload>,
    ) -> bool {
        for &key in keys {
            let cached = atlas.glyphs.get(&key);
            if cached.is_some_and(|glyph| glyph.resident || glyph.upload_frame == atlas.frame)
                || (cached.is_none() && !retry_new)
            {
                continue;
            }
            let cached = cached.and_then(|glyph| match glyph.image {
                AtlasImage::Placed(placed) => Some(placed),
                _ => None,
            });
            let pixels = match self.render_glyph(key, atlas.size) {
                RenderedGlyph::Pixels(pixels) => pixels,
                RenderedGlyph::Empty => {
                    atlas.insert(key, AtlasImage::Empty);
                    continue;
                }
                RenderedGlyph::Oversized => {
                    atlas.insert(key, AtlasImage::Oversized);
                    continue;
                }
                RenderedGlyph::RasterOnly => {
                    atlas.insert(key, AtlasImage::RasterOnly);
                    continue;
                }
            };
            if pixels.size.iter().any(|side| *side > atlas.size) {
                atlas.insert(key, AtlasImage::Oversized);
                continue;
            }
            let placed = match cached {
                Some(placed) => placed,
                None => {
                    let Some((slot, origin)) = atlas.allocate(pixels.size) else {
                        return false;
                    };
                    PlacedGlyph {
                        slot,
                        origin,
                        size: pixels.size,
                        left: pixels.left,
                        top: pixels.top,
                        mode: pixels.mode,
                    }
                }
            };
            uploads.push(GlyphUpload {
                origin: placed.origin,
                size: placed.size,
                pixels: pixels.pixels,
            });
            atlas.insert(key, AtlasImage::Placed(placed));
        }
        true
    }

    fn render_glyph(&mut self, key: AtlasKey, max_size: u32) -> RenderedGlyph {
        match key {
            AtlasKey::Raster(key) => {
                if self.is_plain_outline(key)
                    && let Some(commands) = self
                        .swash_cache
                        .get_outline_commands_uncached(&mut self.font_system, key)
                {
                    let mut oversized = false;
                    swash::zeno::Mask::new(&commands[..])
                        .origin(swash::zeno::Origin::BottomLeft)
                        .offset(swash::zeno::Vector::new(
                            key.x_bin.as_float(),
                            key.y_bin.as_float(),
                        ))
                        .inspect(|_, width, height| {
                            oversized = width > max_size || height > max_size
                        });
                    if oversized {
                        return RenderedGlyph::Oversized;
                    }
                }
                self.swash_cache
                    .get_image_uncached(&mut self.font_system, key)
                    .filter(|image| {
                        image.placement.width > 0
                            && image.placement.height > 0
                            && image.data.iter().any(|&byte| byte != 0)
                    })
                    .map_or(RenderedGlyph::Empty, |image| {
                        RenderedGlyph::Pixels(raster_pixels(&image, key))
                    })
            }
            AtlasKey::RasterStroke(key, half_width) => {
                self.render_stroke(key, f32::from_bits(half_width), max_size)
            }
            #[cfg(feature = "msdf")]
            AtlasKey::Msdf(key) => msdf::render_msdf_glyph(
                &mut self.font_system,
                &mut self.swash_cache,
                key,
                None,
                max_size,
            ),
            #[cfg(feature = "msdf")]
            AtlasKey::Sdf(key, tier) => msdf::render_msdf_glyph(
                &mut self.font_system,
                &mut self.swash_cache,
                key,
                Some(tier),
                max_size,
            ),
        }
    }

    /// A plain outline stroked `half_width` pixels to each side with round joins, placed as
    /// Swash places its fill.
    fn render_stroke(&mut self, key: CacheKey, half_width: f32, max_size: u32) -> RenderedGlyph {
        use swash::zeno::{Join, Mask, Origin, Stroke, Vector};
        if half_width == 0.0 || !self.is_plain_outline(key) {
            return RenderedGlyph::Empty;
        }
        let Some(commands) = self
            .swash_cache
            .get_outline_commands_uncached(&mut self.font_system, key)
        else {
            return RenderedGlyph::Empty;
        };
        let offset = Vector::new(key.x_bin.as_float(), key.y_bin.as_float());
        let mut stroke = Stroke::new(2.0 * half_width);
        stroke.join(Join::Round);
        let mut mask = Mask::new(&commands[..]);
        let mut size = [0; 2];
        mask.style(stroke)
            .origin(Origin::BottomLeft)
            .offset(offset)
            .render_offset(offset)
            .inspect(|_, width, height| size = [width, height]);
        if size.iter().any(|side| *side > max_size) {
            return RenderedGlyph::Oversized;
        }
        if size.contains(&0) {
            return RenderedGlyph::Empty;
        }
        let (alpha, placement) = mask.render();
        RenderedGlyph::Pixels(GlyphPixels {
            size: [placement.width, placement.height],
            left: placement.left as f32,
            top: placement.top as f32,
            mode: GLYPH_MODE_RASTER_MASK,
            pixels: alpha.into_iter().flat_map(|alpha| [alpha; 4]).collect(),
        })
    }
}

/// The half width below which a raster stroke's coverage at the outline, twice its half
/// width, rounds to nothing in 8-bit alpha. Distance-field strokes fade out continuously
/// instead, as they may be magnified.
const MIN_STROKE_HALF_WIDTH: f32 = 1.0 / 512.0;

/// The half width of the stroke `strokes` gives `glyph`'s span, if it has one.
fn glyph_stroke(strokes: &[f32], glyph: &TextGlyph) -> Option<f32> {
    strokes
        .get(glyph.item.span as usize)
        .copied()
        .filter(|half_width| *half_width > 0.0)
}

/// How far past the outline a stroke `half_width` pixels to each side may draw: further than
/// `half_width` where its raster stroke rounds it up.
pub fn stroke_reach(half_width: f32) -> f32 {
    half_width.max(f32::from_bits(stroke_width_key(half_width)))
}

/// The bits of `half_width` rounded for a raster stroke's key: to quarter pixels from 0.25 px
/// to 4 px, and otherwise to a sixteenth of its power of two, so animated widths reuse
/// strokes. Below a quarter pixel a stroke's coverage follows its width, which the relative
/// rounding keeps from jumping, down to [`MIN_STROKE_HALF_WIDTH`], below which it is 0.
fn stroke_width_key(half_width: f32) -> u32 {
    if half_width < MIN_STROKE_HALF_WIDTH {
        0
    } else if (0.25..4.0).contains(&half_width) {
        ((half_width * 4.0).round() / 4.0).to_bits()
    } else {
        (half_width.to_bits() + (1 << 18)) & !((1 << 19) - 1)
    }
}

impl Default for TextSystem {
    fn default() -> Self {
        Self::new()
    }
}

/// `[min_x, min_y, max_x, max_y]` of an outline's control points, or `None` when it is empty.
fn outline_bounds(commands: &[swash::zeno::Command]) -> Option<[f32; 4]> {
    use swash::zeno::Command;
    let mut bounds: Option<[f32; 4]> = None;
    let mut include = |point: swash::zeno::Vector| {
        bounds = Some(match bounds {
            Some([min_x, min_y, max_x, max_y]) => [
                min_x.min(point.x),
                min_y.min(point.y),
                max_x.max(point.x),
                max_y.max(point.y),
            ],
            None => [point.x, point.y, point.x, point.y],
        });
    };
    for command in commands {
        match *command {
            Command::MoveTo(p) | Command::LineTo(p) => include(p),
            Command::QuadTo(c, p) => {
                include(c);
                include(p);
            }
            Command::CurveTo(c0, c1, p) => {
                include(c0);
                include(c1);
                include(p);
            }
            Command::Close => {}
        }
    }
    bounds
}

fn load_default_fonts(font_system: &mut FontSystem) {
    let db = font_system.db_mut();
    db.load_font_data(ROBOTO_REGULAR_BYTES.to_vec());
    db.set_sans_serif_family(DEFAULT_FONT_FAMILY);
}

/// A run of text with one font and spacing.
#[derive(Debug, Clone, PartialEq)]
pub struct TextSpan {
    pub text: String,
    pub font_family: String,
    pub font_size: f32,
    pub font_weight: u16,
    pub font_style: TextFontStyle,
    /// Extra advance in pixels after every glyph. May be negative.
    pub letter_spacing: f32,
    /// OpenType features to shape with. Text of the same word takes the features of the first
    /// span that starts it.
    pub font_features: Vec<FontFeature>,
    /// Case applied to the text before shaping. Everything the shaped text reports, such as
    /// byte ranges, refers to the transformed text.
    pub transform: TextTransform,
    /// Pixels the span's glyphs sit above the baseline. The line keeps its height.
    pub baseline_shift: f32,
}

/// An OpenType feature setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FontFeature {
    /// Four ASCII characters, such as `*b"tnum"`.
    pub tag: [u8; 4],
    /// 0 is off and 1 is on; larger values select an alternate.
    pub value: u32,
}

/// A case mapping of a span's text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextTransform {
    #[default]
    None,
    Uppercase,
    Lowercase,
    /// Uppercases the first letter or digit of each word, where words are separated by
    /// whitespace and continue across spans.
    Capitalize,
}

/// The text of each span after its transform. Untransformed text is borrowed.
fn transformed_texts(spans: &[TextSpan]) -> Vec<Cow<'_, str>> {
    let joined: String = if spans
        .iter()
        .any(|span| span.transform == TextTransform::Lowercase)
    {
        spans.iter().map(|span| span.text.as_str()).collect()
    } else {
        String::new()
    };
    let mut start = 0;
    let mut at_word_start = true;
    spans
        .iter()
        .map(|span| {
            let span_start = start;
            start += span.text.len();
            let mut capitalized = String::new();
            let capitalize = span.transform == TextTransform::Capitalize;
            for char in span.text.chars() {
                let starts_word = at_word_start && char.is_alphanumeric();
                if char.is_whitespace() {
                    at_word_start = true;
                } else if char.is_alphanumeric() {
                    at_word_start = false;
                }
                if capitalize {
                    if starts_word {
                        capitalized.extend(char.to_uppercase());
                    } else {
                        capitalized.push(char);
                    }
                }
            }
            match span.transform {
                TextTransform::None => Cow::Borrowed(span.text.as_str()),
                TextTransform::Uppercase => Cow::Owned(span.text.to_uppercase()),
                TextTransform::Lowercase => {
                    Cow::Owned(lowercase_in_context(&joined, span_start, &span.text))
                }
                TextTransform::Capitalize => Cow::Owned(capitalized),
            }
        })
        .collect()
}

/// Lowercases `text`, which starts at byte `start` of `joined`, like `str::to_lowercase` on
/// `joined`: a capital sigma is final when a cased letter precedes it and none follows it,
/// ignoring case-ignorable characters, even when that context is in a neighbouring span.
fn lowercase_in_context(joined: &str, start: usize, text: &str) -> String {
    let mut lowered = String::with_capacity(text.len());
    for (offset, char) in text.char_indices() {
        if char != '\u{3A3}' {
            lowered.extend(char.to_lowercase());
            continue;
        }
        let at = start + offset;
        let is_final = cased_after_ignorable(joined[..at].chars().rev())
            && !cased_after_ignorable(joined[at + char.len_utf8()..].chars());
        lowered.push(if is_final { '\u{3C2}' } else { '\u{3C3}' });
    }
    lowered
}

fn cased_after_ignorable(mut chars: impl Iterator<Item = char>) -> bool {
    chars
        .find(|&char| !is_case_ignorable(char))
        .is_some_and(is_cased)
}

// `char` has no stable cased or case-ignorable predicates, so ask `str::to_lowercase`, whose
// final-sigma rule is defined by them.
fn is_cased(char: char) -> bool {
    format!("{char}\u{3A3}").to_lowercase().ends_with('\u{3C2}')
}

fn is_case_ignorable(char: char) -> bool {
    !is_cased(char)
        && format!("A{char}\u{3A3}")
            .to_lowercase()
            .ends_with('\u{3C2}')
}

impl TextSpan {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            font_family: DEFAULT_FONT_FAMILY.to_string(),
            font_size: 16.0,
            font_weight: 400,
            font_style: TextFontStyle::Normal,
            letter_spacing: 0.0,
            font_features: Vec::new(),
            transform: TextTransform::None,
            baseline_shift: 0.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TextLayoutRequest {
    /// Text in order, as styled runs that may span lines and paragraphs.
    pub spans: Vec<TextSpan>,
    pub max_width: Option<f32>,
    /// Line pitch in pixels; `None` is 1.2 × the largest font size on each line.
    pub line_height: Option<f32>,
    /// Extra space in pixels after each paragraph but the last.
    pub paragraph_spacing: f32,
    pub align: TextAlign,
    pub origin: [f32; 2],
    pub color: [f32; 4],
    pub disable_hinting: bool,
    /// Most lines to show.
    pub max_lines: Option<u32>,
    /// Most height of the lines to show.
    pub max_height: Option<f32>,
    /// How the last line shown ends when lines past `max_lines` or `max_height` are dropped.
    pub overflow: TextOverflow,
    /// When set, every size shrinks by the largest factor down to this one that fits the text
    /// in `max_width`, `max_lines` and `max_height` without breaking a word, before lines are
    /// dropped. Font sizes, letter spacing, `line_height`, baseline shifts, paragraph spacing and
    /// the decorations' own sizes all scale.
    pub min_fit_scale: Option<f32>,
}

impl TextLayoutRequest {
    /// Plain text in one default span.
    pub fn new(content: impl Into<String>) -> Self {
        Self::from_spans(vec![TextSpan::new(content)])
    }

    pub fn from_spans(spans: Vec<TextSpan>) -> Self {
        Self {
            spans,
            max_width: None,
            line_height: None,
            paragraph_spacing: 0.0,
            align: TextAlign::Left,
            origin: [0.0, 0.0],
            color: [1.0, 1.0, 1.0, 1.0],
            disable_hinting: true,
            max_lines: None,
            max_height: None,
            overflow: TextOverflow::Clip,
            min_fit_scale: None,
        }
    }

    /// Placement and color cannot affect shaping, fitting or measurement.
    fn cache_key(&self) -> Self {
        Self {
            origin: [0.0; 2],
            color: [1.0; 4],
            ..self.clone()
        }
    }

    fn is_limited(&self) -> bool {
        self.max_width.is_some_and(|width| width > 0.0)
            || self.max_lines.is_some()
            || self.max_height.is_some()
    }

    /// This request with every size scaled by `scale`.
    fn scaled(&self, scale: f32) -> Self {
        let mut request = self.clone();
        for span in &mut request.spans {
            span.font_size *= scale;
            span.letter_spacing *= scale;
            span.baseline_shift *= scale;
        }
        request.line_height = self.line_height.map(|height| height * scale);
        request.paragraph_spacing *= scale;
        request
    }

    /// 1 when `shaped`, shaped without breaking words, fits the limits, and otherwise an estimate
    /// below 1 of the factor that would make it fit. Wrapped lines hold more text as it shrinks,
    /// so wrapped height shrinks about as the square of the factor.
    fn fit_ratio(&self, shaped: &ShapedText) -> f32 {
        let wraps = self.max_width.is_some_and(|width| width > 0.0);
        let TextMeasurement { width, height } = shaped.measurement;
        let mut ratio: f32 = 1.0;
        if let Some(max) = self.max_width.filter(|_| wraps)
            && width > max + FIT_TOLERANCE
        {
            ratio = ratio.min(max / width);
        }
        // Shrinking never joins paragraphs, so lines past them are dropped at any size.
        if let Some(max) = self.max_lines.map(|max| max.max(shaped.paragraphs))
            && shaped.counts.line > max
        {
            let lines = max as f32 / shaped.counts.line as f32;
            ratio = ratio.min(if wraps { lines } else { 0.0 });
        }
        if let Some(max) = self.max_height
            && height > max + FIT_TOLERANCE
        {
            let heights = max.max(0.0) / height;
            ratio = ratio.min(if wraps { heights.sqrt() } else { heights });
        }
        ratio
    }

    fn span_metrics(&self, span: &TextSpan) -> Metrics {
        let font_size = span.font_size.max(1.0);
        Metrics::new(
            font_size,
            self.line_height.unwrap_or(font_size * 1.2).max(1.0),
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextFontStyle {
    Normal,
    Italic,
    Oblique,
}

/// How the last line shown ends when lines past a request's `max_lines` or `max_height` are
/// dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextOverflow {
    /// As it is.
    #[default]
    Clip,
    /// In an ellipsis in reading order, in the style of the text it follows. Clusters at the
    /// line's end are dropped until it fits `max_width`. The ellipsis is one glyph, numbered
    /// after the last one shown.
    Ellipsis,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextAlign {
    Left,
    Center,
    Right,
    Justified,
}

/// Where a glyph lies in its text, for animating glyphs, words or lines apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TextItem {
    /// Character cluster, in logical order and counting whitespace. Ligatures are one
    /// cluster, and combining marks share their base's.
    pub glyph: u32,
    /// Whitespace-separated word. Whitespace belongs to the word before it.
    pub word: u32,
    /// Line after wrapping, counting empty lines.
    pub line: u32,
    /// Index of the glyph's span in the request.
    pub span: u32,
}

/// Numbers the words of a text's paragraphs, which arrive in order.
#[derive(Debug, Default)]
struct WordCounter {
    paragraph: Option<usize>,
    /// Words in the paragraphs before the current one.
    before: u32,
    /// Byte offsets where the current paragraph's words start.
    starts: Vec<usize>,
}

impl WordCounter {
    fn enter(&mut self, paragraph: usize, text: &str) {
        if self.paragraph == Some(paragraph) {
            return;
        }
        self.before = self.total();
        self.paragraph = Some(paragraph);
        self.starts.clear();
        let mut previous_space = true;
        for (offset, char) in text.char_indices() {
            if previous_space && !char.is_whitespace() {
                self.starts.push(offset);
            }
            previous_space = char.is_whitespace();
        }
    }

    /// The word of the cluster `text` that starts at byte `start` of the paragraph.
    fn word_at(&self, text: &str, start: usize) -> u32 {
        let offset = start + text.find(|char: char| !char.is_whitespace()).unwrap_or(0);
        let started = self.starts.partition_point(|&word| word <= offset);
        self.before + started.saturating_sub(1) as u32
    }

    fn total(&self) -> u32 {
        self.before + self.starts.len() as u32
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextMeasurement {
    pub width: f32,
    pub height: f32,
}

/// Shaped text relative to a `[0, 0]` origin.
#[derive(Debug, Clone)]
pub struct ShapedText {
    pub measurement: TextMeasurement,
    /// Advance box of all glyphs as `[x, y, width, height]`.
    pub bounds: [f32; 4],
    /// Number of glyphs, words, lines and spans.
    pub counts: TextItem,
    /// Each contiguous run of one span and direction on a line, by line and then from left
    /// to right. An ellipsis is a segment of its own, covering no bytes.
    pub segments: Vec<TextSegment>,
    /// The box of each span's segments as `[x, y, width, height]`, or `None` for a span that
    /// draws none.
    pub span_boxes: Vec<Option<[f32; 4]>>,
    /// Decoration metrics of each span's font, or zero for spans that draw no glyphs.
    pub span_metrics: Vec<TextSpanMetrics>,
    /// Factor every size was scaled by to fit the request's limits; 1 unless it shrank.
    pub fit_scale: f32,
    /// Byte offset of each span in the whole text, and the text's length.
    span_starts: Vec<usize>,
    /// Lines the text has however wide its lines may be.
    paragraphs: u32,
    glyphs: Vec<ShapedGlyph>,
}

/// Part of a span on one line, relative to the text's origin.
#[derive(Debug, Clone, PartialEq)]
pub struct TextSegment {
    pub span: u32,
    pub line: u32,
    /// Left and right edges of the glyph advances, without the line's trailing tracking.
    pub x: [f32; 2],
    /// Where the span's glyphs sit, which its baseline shift has moved off the line's.
    pub baseline: f32,
    /// Pixels the span's baseline is above the line's.
    pub shift: f32,
    pub line_top: f32,
    pub line_height: f32,
    /// Largest ascent and descent of the segment's fonts, as distances from the baseline.
    pub ascent: f32,
    pub descent: f32,
    /// Bytes of the span's transformed text the segment covers, in reading order. They run from
    /// `x[1]` to `x[0]` when `rtl`.
    pub bytes: Range<usize>,
    pub rtl: bool,
}

/// A span's decoration strokes in pixels. Offsets are the Y of each stroke's top edge
/// below the baseline, so a strikeout's is negative.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct TextSpanMetrics {
    pub underline_offset: f32,
    pub underline_thickness: f32,
    pub strikeout_offset: f32,
    pub strikeout_thickness: f32,
}

/// A decoration drawn along a run of consecutive spans.
#[derive(Debug, Clone, PartialEq)]
pub struct TextDecoration {
    pub kind: TextDecorationKind,
    /// The spans of the request it runs along.
    pub spans: Range<usize>,
    /// Whether the decoration moves with the baseline shift of the spans, or else stays on
    /// the line's baseline.
    pub shifted: bool,
    /// Start and end along the run's text in reading order, as fractions of its width: the
    /// widths of its lines added together, so it continues from line to line. Within a line it
    /// follows the text's logical order, with an ellipsis last.
    pub extent: [f32; 2],
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TextDecorationKind {
    /// A box behind each line of the run, covering the largest ascent and descent of its spans
    /// on the line, and grown by `padding`.
    Highlight {
        padding: [f32; 2],
        radius: f32,
    },
    Underline(TextLineDecoration),
    Strikethrough(TextLineDecoration),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextLineDecoration {
    pub style: TextLineStyle,
    /// Stroke thickness in pixels; `None` takes that of the run's tallest span on each line.
    pub thickness: Option<f32>,
    /// Y of the stroke's top edge below the baseline; `None` takes that of the run's tallest
    /// span on each line.
    pub offset: Option<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextLineStyle {
    Solid,
    Double,
    Dotted,
    Dashed,
    Wavy,
}

/// A font's metrics in em, with Y pointing down from the baseline.
#[derive(Debug, Clone, Copy, Default)]
struct FontMetrics {
    ascent: f32,
    descent: f32,
    decorations: TextSpanMetrics,
}

impl FontMetrics {
    /// Missing decorations take cosmic-text's defaults.
    fn new(font: &cosmic_text::Font) -> Self {
        let metrics = font.metrics();
        let upem = f32::from(metrics.units_per_em);
        if upem == 0.0 {
            return Self::default();
        }
        let [underline, strikeout] =
            [(metrics.underline, -0.125), (metrics.strikeout, 0.3)].map(|(decoration, offset)| {
                decoration.map_or([-offset, 1.0 / 14.0], |decoration| {
                    [-decoration.offset / upem, decoration.thickness / upem]
                })
            });
        Self {
            ascent: metrics.ascent / upem,
            descent: -metrics.descent / upem,
            decorations: TextSpanMetrics {
                underline_offset: underline[0],
                underline_thickness: underline[1],
                strikeout_offset: strikeout[0],
                strikeout_thickness: strikeout[1],
            },
        }
    }

    fn decorations(&self, font_size: f32) -> TextSpanMetrics {
        let metrics = self.decorations;
        TextSpanMetrics {
            underline_offset: metrics.underline_offset * font_size,
            underline_thickness: metrics.underline_thickness * font_size,
            strikeout_offset: metrics.strikeout_offset * font_size,
            strikeout_thickness: metrics.strikeout_thickness * font_size,
        }
    }
}

#[derive(Debug, Clone)]
struct ShapedGlyph {
    glyph: LayoutGlyph,
    line_y: f32,
    item: TextItem,
    pivot: [f32; 2],
}

impl ShapedText {
    /// Number of glyphs, whitespace included, that [`Self::place`] returns.
    pub fn glyph_count(&self) -> usize {
        self.glyphs.len()
    }

    /// Places glyphs at `origin`. The fractional origin selects each glyph's
    /// subpixel bins, and raster glyph positions are snapped to whole pixels.
    pub fn place(&self, origin: [f32; 2], color: [f32; 4]) -> TextLayout {
        let glyphs = self
            .glyphs
            .iter()
            .map(|shaped| {
                let glyph = &shaped.glyph;
                let position = [
                    glyph.x + glyph.font_size * glyph.x_offset + origin[0],
                    glyph.y - glyph.font_size * glyph.y_offset + (origin[1] + shaped.line_y),
                ];
                // Swash offsets outlines with Y pointing up; layout positions point down.
                let (cache_key, x, y) = CacheKey::new(
                    glyph.font_id,
                    glyph.glyph_id,
                    glyph.font_size,
                    (position[0], -position[1]),
                    glyph.font_weight,
                    glyph.cache_key_flags,
                );
                TextGlyph {
                    key: GlyphKey(cache_key),
                    x: x as f32,
                    y: -y as f32,
                    position,
                    color,
                    item: shaped.item,
                    pivot: [shaped.pivot[0] + origin[0], shaped.pivot[1] + origin[1]],
                }
            })
            .collect();
        TextLayout {
            measurement: self.measurement,
            counts: self.counts,
            bounds: [
                origin[0] + self.bounds[0],
                origin[1] + self.bounds[1],
                self.bounds[2],
                self.bounds[3],
            ],
            glyphs,
        }
    }

    /// Appends the rect instances that draw `decoration` with the text placed at `origin`,
    /// filled with the paint index `paint` gives each span. Each line the decoration reaches
    /// takes one rect, split only where the paint changes or right-to-left text breaks the
    /// extent apart.
    pub fn decorate(
        &self,
        decoration: &TextDecoration,
        origin: [f32; 2],
        paint: impl Fn(usize) -> u32,
        instances: &mut Vec<GpuGlyphInstance>,
    ) {
        let last = self.span_starts.len() - 1;
        let spans = decoration.spans.start.min(last)..decoration.spans.end.min(last);
        let width = |segment: &TextSegment| segment.x[1] - segment.x[0];
        let run_width: f32 = self
            .segments
            .iter()
            .filter(|segment| spans.contains(&(segment.span as usize)))
            .map(width)
            .sum();
        let [start, end] = decoration
            .extent
            .map(|fraction| fraction.clamp(0.0, 1.0) * run_width);
        if start >= end {
            return;
        }
        // How far along the run the current line starts.
        let mut line_start = 0.0;
        for line in self.segments.chunk_by(|a, b| a.line == b.line) {
            let in_run = || {
                line.iter()
                    .filter(|segment| spans.contains(&(segment.span as usize)))
            };
            // Distances run in logical order, and the rects are then emitted left to right.
            let segments: Vec<&TextSegment> = in_run().collect();
            let mut logical: Vec<usize> = (0..segments.len()).collect();
            logical.sort_by_key(|&index| {
                let segment = segments[index];
                (segment.bytes.is_empty(), segment.span, segment.bytes.start)
            });
            let mut from = vec![0.0; segments.len()];
            for index in logical {
                from[index] = line_start;
                line_start += width(segments[index]);
            }
            let Some(tallest) = in_run().max_by(|a, b| a.ascent.total_cmp(&b.ascent)) else {
                continue;
            };
            let metrics = self.span_metrics[tallest.span as usize];
            let baseline = if decoration.shifted {
                tallest.baseline
            } else {
                tallest.baseline + tallest.shift
            };
            let (top, bottom, shape, padding, radius) = match decoration.kind {
                TextDecorationKind::Highlight { padding, radius } => {
                    let (padding, radius) = (
                        padding.map(|pad| pad * self.fit_scale),
                        radius * self.fit_scale,
                    );
                    let ascent = in_run().map(|segment| segment.ascent).fold(0.0, f32::max);
                    let descent = in_run().map(|segment| segment.descent).fold(0.0, f32::max);
                    let top = baseline - ascent - padding[1];
                    (
                        top,
                        baseline + descent + padding[1],
                        RECT_BOX,
                        padding[0],
                        radius,
                    )
                }
                TextDecorationKind::Underline(line) | TextDecorationKind::Strikethrough(line) => {
                    let (offset, thickness) = match decoration.kind {
                        TextDecorationKind::Underline(_) => {
                            (metrics.underline_offset, metrics.underline_thickness)
                        }
                        _ => (metrics.strikeout_offset, metrics.strikeout_thickness),
                    };
                    let thickness = line
                        .thickness
                        .map_or(thickness, |thickness| thickness * self.fit_scale);
                    if thickness <= 0.0 {
                        continue;
                    }
                    let top =
                        baseline + line.offset.map_or(offset, |offset| offset * self.fit_scale);
                    let shape = match line.style {
                        TextLineStyle::Solid => RECT_SOLID,
                        TextLineStyle::Double => RECT_DOUBLE,
                        TextLineStyle::Dotted => RECT_DOTTED,
                        TextLineStyle::Dashed => RECT_DASHED,
                        TextLineStyle::Wavy => RECT_WAVY,
                    };
                    (top, top + thickness, shape, 0.0, 0.0)
                }
            };
            // Patterns start where the run does on the line, so an extent drawing in leaves
            // them still.
            let phase = origin[0]
                + in_run()
                    .map(|segment| segment.x[0])
                    .fold(f32::INFINITY, f32::min);
            let mut emit = |x: [f32; 2], paint: u32, joins: u32| {
                let pad = |joined: u32| if joins & joined == 0 { padding } else { 0.0 };
                let [x0, x1] = [x[0] - pad(RECT_JOINED_LEFT), x[1] + pad(RECT_JOINED_RIGHT)];
                instances.push(GpuGlyphInstance::rect(
                    [origin[0] + x0, origin[1] + top, x1 - x0, bottom - top],
                    shape,
                    joins,
                    radius,
                    phase,
                    paint,
                ));
            };
            // The rect being grown, its paint and its `RECT_JOINED_*` flags so far.
            let mut current: Option<([f32; 2], u32, u32)> = None;
            for (segment, &from) in segments.iter().zip(&from) {
                let along = [start.max(from), end.min(from + width(segment))].map(|at| at - from);
                if along[0] >= along[1] {
                    continue;
                }
                let x = if segment.rtl {
                    [segment.x[1] - along[1], segment.x[1] - along[0]]
                } else {
                    [segment.x[0] + along[0], segment.x[0] + along[1]]
                };
                let paint = paint(segment.span as usize);
                current = Some(match current {
                    Some((range, current, joins))
                        if x[0] <= range[1] + 0.01 && x[1] >= range[0] - 0.01 =>
                    {
                        if current == paint {
                            ([range[0].min(x[0]), range[1].max(x[1])], paint, joins)
                        } else {
                            emit(range, current, joins | RECT_JOINED_RIGHT);
                            (x, paint, RECT_JOINED_LEFT)
                        }
                    }
                    Some((range, current, joins)) => {
                        emit(range, current, joins);
                        (x, paint, 0)
                    }
                    None => (x, paint, 0),
                });
            }
            if let Some((range, paint, joins)) = current {
                emit(range, paint, joins);
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TextLayout {
    pub measurement: TextMeasurement,
    /// Advance box of all glyphs as `[x, y, width, height]`.
    pub bounds: [f32; 4],
    /// Number of glyphs, words and lines.
    pub counts: TextItem,
    pub glyphs: Vec<TextGlyph>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TextGlyph {
    /// Raster key, including the glyph's subpixel bin.
    pub key: GlyphKey,
    /// Whole-pixel pen position; the subpixel remainder is carried by `key`.
    pub x: f32,
    pub y: f32,
    /// Exact pen position, used by distance-field glyphs.
    pub position: [f32; 2],
    pub color: [f32; 4],
    pub item: TextItem,
    /// Centre of the glyph's cluster: the middle of its advance and of its line.
    pub pivot: [f32; 2],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub struct GlyphKey(pub CacheKey);

/// etagere indexes its items with `u16` and wraps silently past that. Each
/// allocation uses at most two items plus one per shelf, so this keeps atlases
/// up to 4096 pixels tall well within range.
const MAX_ATLAS_GLYPHS: usize = 16 * 1024;

/// Glyph atlas shared by every text layout drawn with one GPU texture.
///
/// Raster glyphs and distance fields share one RGBA8 texture. Only placement
/// is kept on the CPU; glyph pixels exist solely in the uploads returned for
/// a frame. A fixed size keeps every free rectangle available to large glyphs.
pub struct GlyphAtlas {
    allocator: etagere::AtlasAllocator,
    allocations: usize,
    size: u32,
    availability: u64,
    glyphs: foldhash::HashMap<AtlasKey, AtlasGlyph>,
    frame: u64,
    layouts_this_frame: usize,
    /// Eviction candidates for this frame, most recently used first.
    stale: Option<Vec<AtlasKey>>,
    /// Counts inserted `Oversized` and `RasterOnly` markers, which change [`Self::key_for`].
    markers: u64,
}

/// Raster glyphs are keyed by size and subpixel bin; distance fields only by
/// font, glyph, weight and fake italic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum AtlasKey {
    Raster(CacheKey),
    /// A raster glyph's outline stroked to each side by the half width whose bits
    /// [`stroke_width_key`] gives.
    RasterStroke(CacheKey, u32),
    #[cfg(feature = "msdf")]
    Msdf(CacheKey),
    /// A true distance field, which strokes are drawn from, with the range tier from
    /// `msdf::stroke_key`.
    #[cfg(feature = "msdf")]
    Sdf(CacheKey, u32),
}

impl AtlasKey {
    /// The glyph key; a distance field's holds the field's texels per em as its font size.
    fn cache_key(self) -> CacheKey {
        match self {
            Self::Raster(key) | Self::RasterStroke(key, _) => key,
            #[cfg(feature = "msdf")]
            Self::Msdf(key) | Self::Sdf(key, _) => key,
        }
    }
}

#[derive(Debug)]
struct AtlasGlyph {
    image: AtlasImage,
    last_used: u64,
    upload_frame: u64,
    resident: bool,
}

#[derive(Debug, Clone, Copy)]
enum AtlasImage {
    /// Nothing to draw, such as whitespace.
    Empty,
    /// A distance-field key whose glyph is drawn as raster instead.
    RasterOnly,
    Oversized,
    Placed(PlacedGlyph),
}

#[derive(Debug, Clone, Copy)]
struct PlacedGlyph {
    slot: AllocId,
    origin: [u32; 2],
    size: [u32; 2],
    /// Image offset from the pen, y up; in pixels for raster glyphs and field
    /// texels for distance fields.
    left: f32,
    top: f32,
    mode: u32,
}

impl std::fmt::Debug for GlyphAtlas {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GlyphAtlas")
            .field("size", &self.size)
            .field("glyphs", &self.glyphs)
            .finish_non_exhaustive()
    }
}

impl GlyphAtlas {
    pub fn new(size: u32) -> Self {
        let size = size.clamp(1, 4096);
        Self {
            allocator: etagere::AtlasAllocator::new(size2(size as i32, size as i32)),
            allocations: 0,
            size,
            availability: 0,
            glyphs: foldhash::HashMap::default(),
            frame: 0,
            layouts_this_frame: 0,
            stale: None,
            markers: 0,
        }
    }

    /// Current width and height of the atlas texture.
    pub fn size(&self) -> u32 {
        self.size
    }

    /// Starts a frame. Glyphs used by any layout prepared in the same frame are
    /// not evicted for each other.
    pub fn begin_frame(&mut self) -> u64 {
        let previous = self.frame;
        // Retry suppressed layouts when another layout stops pinning glyphs.
        if self.glyphs.values().any(|glyph| {
            matches!(glyph.image, AtlasImage::Placed(_)) && glyph.last_used + 1 == previous
        }) {
            self.availability += 1;
        }
        self.glyphs.retain(|_, glyph| {
            glyph.last_used == previous || matches!(glyph.image, AtlasImage::Placed(_))
        });
        self.layouts_this_frame = 0;
        self.stale = None;
        self.frame += 1;
        self.frame
    }

    /// Marks the uploads of `frame` as written to the GPU texture.
    pub fn commit(&mut self, frame: u64) {
        for glyph in self.glyphs.values_mut() {
            if glyph.upload_frame == frame {
                glyph.resident = true;
            }
        }
    }

    /// Number of cached glyphs, including empty ones.
    pub fn len(&self) -> usize {
        self.glyphs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.glyphs.is_empty()
    }

    /// Whether `key` holds a marker of the given kind, which routes the keys it is read for.
    /// It counts as used this frame.
    fn mark_used(&mut self, key: &AtlasKey, oversized: bool) -> bool {
        let Some(glyph) = self.glyphs.get_mut(key) else {
            return false;
        };
        let marker = match glyph.image {
            AtlasImage::Oversized => oversized,
            AtlasImage::RasterOnly => !oversized,
            _ => false,
        };
        if marker {
            glyph.last_used = self.frame;
        }
        marker
    }

    /// Distance fields are used where requested, and for raster glyphs too
    /// large for the atlas, unless the glyph can only be rasterized.
    fn key_for(&mut self, key: GlyphKey, mode: TextRenderMode) -> AtlasKey {
        let raster = AtlasKey::Raster(key.0);
        let oversized = self.mark_used(&raster, true);
        #[cfg(feature = "msdf")]
        {
            let msdf = AtlasKey::Msdf(msdf::msdf_key(key.0));
            let wanted = match mode {
                TextRenderMode::Raster => false,
                TextRenderMode::Msdf => true,
                TextRenderMode::Auto => f32::from_bits(key.0.font_size_bits) >= MSDF_AUTO_MIN_SIZE,
            } || oversized;
            if wanted && !self.mark_used(&msdf, false) {
                return msdf;
            }
        }
        #[cfg(not(feature = "msdf"))]
        let _ = (mode, oversized);
        raster
    }

    /// The key of the stroke `half_width` pixels to each side of `key`'s outline: a field
    /// where the glyph draws one, or where its stroked raster outline is too large for the
    /// atlas, unless the outline can only be rasterized.
    fn stroke_key_for(&mut self, key: GlyphKey, mode: TextRenderMode, half_width: f32) -> AtlasKey {
        let raster = AtlasKey::RasterStroke(key.0, stroke_width_key(half_width));
        let oversized = self.mark_used(&raster, true);
        #[cfg(feature = "msdf")]
        {
            let (field, tier) = msdf::stroke_key(key.0, half_width);
            let field = AtlasKey::Sdf(field, tier);
            let wanted = matches!(self.key_for(key, mode), AtlasKey::Msdf(_)) || oversized;
            if wanted && !self.mark_used(&field, false) {
                return field;
            }
        }
        #[cfg(not(feature = "msdf"))]
        let _ = (mode, oversized);
        raster
    }

    /// Unique atlas keys of `glyphs` and their `strokes`, in order. Markers that route the keys
    /// count as used this frame.
    fn keys_for(
        &mut self,
        glyphs: &[TextGlyph],
        mode: TextRenderMode,
        strokes: &[f32],
    ) -> Vec<AtlasKey> {
        let mut seen = foldhash::HashSet::default();
        let mut keys = Vec::new();
        for glyph in glyphs {
            let stroke = glyph_stroke(strokes, glyph)
                .map(|half_width| self.stroke_key_for(glyph.key, mode, half_width));
            for key in [Some(self.key_for(glyph.key, mode)), stroke]
                .into_iter()
                .flatten()
            {
                if seen.insert(key) {
                    keys.push(key);
                }
            }
        }
        keys
    }

    fn touch(&mut self, keys: &[AtlasKey]) {
        for key in keys {
            if let Some(glyph) = self.glyphs.get_mut(key) {
                glyph.last_used = self.frame;
            }
        }
    }

    fn clear(&mut self) {
        self.glyphs.clear();
        self.allocator.clear();
        self.allocations = 0;
        self.availability += 1;
        self.stale = None;
    }

    fn insert(&mut self, key: AtlasKey, image: AtlasImage) {
        if matches!(image, AtlasImage::Oversized | AtlasImage::RasterOnly) {
            self.markers += 1;
        }
        self.glyphs.insert(
            key,
            AtlasGlyph {
                image,
                last_used: self.frame,
                upload_frame: self.frame,
                resident: !matches!(image, AtlasImage::Placed(_)),
            },
        );
    }

    /// Allocates a slot, evicting the least recently used glyphs not used this
    /// frame.
    fn allocate(&mut self, size: [u32; 2]) -> Option<(AllocId, [u32; 2])> {
        let request = size2(size[0].max(1) as i32, size[1].max(1) as i32);
        loop {
            if self.allocations < MAX_ATLAS_GLYPHS
                && let Some(allocation) = self.allocator.allocate(request)
            {
                self.allocations += 1;
                let min = allocation.rectangle.min;
                return Some((allocation.id, [min.x as u32, min.y as u32]));
            }
            if self.stale.is_none() {
                self.stale = Some(self.stale_by_age());
            }
            let key = self.stale.as_mut()?.pop()?;
            // Layouts prepared since the candidates were collected may use it.
            if let Some(AtlasGlyph {
                image: AtlasImage::Placed(placed),
                last_used,
                ..
            }) = self.glyphs.get(&key)
                && *last_used != self.frame
            {
                self.allocator.deallocate(placed.slot);
                self.glyphs.remove(&key);
                self.allocations -= 1;
                self.availability += 1;
            }
        }
    }

    /// Placed glyphs unused this frame, most recently used first.
    fn stale_by_age(&self) -> Vec<AtlasKey> {
        let mut stale: Vec<(u64, AtlasKey)> = self
            .glyphs
            .iter()
            .filter(|(_, glyph)| {
                matches!(glyph.image, AtlasImage::Placed(_)) && glyph.last_used != self.frame
            })
            .map(|(key, glyph)| (glyph.last_used, *key))
            .collect();
        stale.sort_unstable_by_key(|(last_used, _)| std::cmp::Reverse(*last_used));
        stale.into_iter().map(|(_, key)| key).collect()
    }
}

impl PlacedGlyph {
    /// The instance that draws `glyph` with this image, placed under `key`.
    fn instance(&self, glyph: &TextGlyph, key: AtlasKey) -> GpuGlyphInstance {
        let [width, height] = self.size.map(|side| side as f32);
        let [x, y] = self.origin.map(|texel| texel as f32);
        let (rect, uv_rect) = if self.mode == GLYPH_MODE_MSDF {
            // Fields are scaled to the glyph's size at its exact position, and
            // inset by half a texel so filtering stays inside the slot.
            let font_size = f32::from_bits(glyph.key.0.font_size_bits);
            let scale = font_size / f32::from_bits(key.cache_key().font_size_bits);
            (
                [
                    glyph.position[0] + (self.left + 0.5) * scale,
                    glyph.position[1] - (self.top - 0.5) * scale,
                    (width - 1.0) * scale,
                    (height - 1.0) * scale,
                ],
                [x + 0.5, y + 0.5, x + width - 0.5, y + height - 0.5],
            )
        } else {
            (
                [glyph.x + self.left, glyph.y - self.top, width, height],
                [x, y, x + width, y + height],
            )
        };
        let flags = match key {
            #[cfg(feature = "msdf")]
            AtlasKey::Sdf(_, tier) => tier << GLYPH_RANGE_SHIFT,
            _ => 0,
        };
        GpuGlyphInstance {
            rect,
            uv_rect,
            opacity: glyph.color[3],
            partner: 0,
            _padding: [0; 2],
            mode: self.mode,
            flags,
            offset: [0.0; 2],
            linear: [[1.0, 0.0], [0.0, 1.0]],
        }
    }
}

enum RenderedGlyph {
    Pixels(GlyphPixels),
    Empty,
    Oversized,
    #[cfg_attr(not(feature = "msdf"), allow(dead_code))]
    RasterOnly,
}

struct GlyphPixels {
    size: [u32; 2],
    left: f32,
    top: f32,
    mode: u32,
    /// Premultiplied RGBA8 rows; distance fields keep their channels in RGB.
    pixels: Vec<u8>,
}

fn raster_pixels(image: &SwashImage, key: CacheKey) -> GlyphPixels {
    // Swash composites colour outlines premultiplied but decodes colour
    // bitmaps with straight alpha.
    let straight_color = matches!(image.source, Source::ColorBitmap(_));
    let texels = image.placement.width as usize * image.placement.height as usize;
    let pixels = (0..texels)
        .flat_map(|index| match image.content {
            SwashContent::Mask => [image.data[index]; 4],
            SwashContent::SubpixelMask => {
                let rgb = &image.data[index * 4..index * 4 + 3];
                [rgb[0].max(rgb[1]).max(rgb[2]); 4]
            }
            SwashContent::Color => {
                let [r, g, b, a] = image.data[index * 4..index * 4 + 4] else {
                    unreachable!()
                };
                if straight_color {
                    [premultiply(r, a), premultiply(g, a), premultiply(b, a), a]
                } else {
                    [r, g, b, a]
                }
            }
        })
        .collect();
    // Swash draws bitmaps without the key's subpixel bins, so they are
    // shifted to the nearest whole pixel instead.
    let [snap_x, snap_y] = if matches!(image.source, Source::Bitmap(_) | Source::ColorBitmap(_)) {
        [key.x_bin, key.y_bin].map(|bin| bin.as_float().round())
    } else {
        [0.0; 2]
    };
    GlyphPixels {
        size: [image.placement.width, image.placement.height],
        left: image.placement.left as f32 + snap_x,
        top: image.placement.top as f32 + snap_y,
        mode: if image.content == SwashContent::Color {
            GLYPH_MODE_RASTER_COLOR
        } else {
            GLYPH_MODE_RASTER_MASK
        },
        pixels,
    }
}

fn premultiply(channel: u8, alpha: u8) -> u8 {
    ((u32::from(channel) * u32::from(alpha) + 127) / 255) as u8
}

/// Glyph pixels to write into the atlas texture.
#[derive(Debug, Clone)]
pub struct GlyphUpload {
    pub origin: [u32; 2],
    pub size: [u32; 2],
    /// Tightly packed RGBA8 rows.
    pub pixels: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct PreparedGlyphs {
    /// Pass to [`GlyphAtlas::commit`] once `uploads` have been written.
    pub frame: u64,
    pub instances: Vec<GpuGlyphInstance>,
    /// Index in the layout's glyphs of each instance.
    pub sources: Vec<usize>,
    /// Instances that stroke glyph outlines.
    pub strokes: Vec<GpuGlyphInstance>,
    /// Index in `instances` of the glyph each of `strokes` strokes.
    pub stroke_glyphs: Vec<usize>,
    pub uploads: Vec<GlyphUpload>,
    /// Glyph and stroke instances not drawn because the atlas is full.
    pub dropped_glyphs: usize,
    /// The atlas was cleared and repacked this frame.
    pub compacted: bool,
}

#[cfg(test)]
#[derive(Debug, Clone)]
struct AtlasRender {
    pixels: Vec<u8>,
    instances: Vec<GpuGlyphInstance>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn prepare(
        system: &mut TextSystem,
        atlas: &mut GlyphAtlas,
        layout: &TextLayout,
        mode: TextRenderMode,
    ) -> PreparedGlyphs {
        system.prepare_glyphs(atlas, layout, mode, &mut None)
    }

    #[test]
    fn loads_only_the_bundled_fonts() {
        assert_eq!(TextSystem::new().font_system.db().len(), 1);
    }

    #[test]
    fn measures_roboto_text() {
        let mut system = TextSystem::new();
        let mut request = TextLayoutRequest::new("External URL media");
        request.spans[0].font_size = 44.0;
        request.max_width = Some(900.0);

        let measurement = system.measure(&request);

        assert!(measurement.width > 100.0);
        assert!(measurement.height > 40.0);
    }

    #[test]
    fn repeated_measurement_uses_cache_without_affecting_layout() {
        let mut system = TextSystem::new();
        let mut request = TextLayoutRequest::new("Cached text");
        let measurement = system.measure(&request);
        assert_eq!(system.measure(&request), measurement);
        assert_eq!(system.measurements.len(), 1);

        // A distinctive cached value proves a hit does not shape again.
        let cached = TextMeasurement {
            width: measurement.width + 1.0,
            ..measurement
        };
        system.measurements[0].1 = cached;
        assert_eq!(system.measure(&request), cached);
        request.origin = [24.0, 48.0];
        assert_eq!(system.measure(&request), cached);
        request.color = [0.2, 0.4, 0.6, 0.8];
        assert_eq!(system.measure(&request), cached);
        assert_eq!(system.measurements.len(), 1);

        let layout = system.layout(&request);
        assert_eq!(layout.measurement, measurement);
        assert!(!layout.glyphs.is_empty());
    }

    #[test]
    fn measurement_cache_tracks_spacing_wrapping_limits_and_fit() {
        let mut system = TextSystem::new();
        let mut request =
            TextLayoutRequest::from_spans(vec![span("Hello ", 40.0), span("world\nagain", 40.0)]);
        request.max_width = Some(200.0);
        request.max_lines = Some(2);
        let original = system.measure(&request);
        let mut changed = vec![request.clone(); 7];
        changed[0].spans[0].letter_spacing = 8.0;
        changed[1].max_width = Some(400.0);
        changed[2].max_lines = Some(1);
        changed[3].max_height = Some(48.0);
        changed[4].min_fit_scale = Some(0.1);
        changed[5].overflow = TextOverflow::Ellipsis;
        changed[6].line_height = Some(60.0);
        for changed in changed {
            let measured = system.measure(&changed);
            assert_eq!(measured, system.shape(&changed).measurement, "{changed:?}");
            assert_ne!(measured, original, "{changed:?}");
            assert_eq!(system.measure(&request), original);
        }
        assert_eq!(system.measurements.len(), 8);
    }

    #[test]
    fn measurement_cache_evicts_the_oldest_request() {
        let mut system = TextSystem::new();
        let first = TextLayoutRequest::new("First");
        system.measure(&first);
        for index in 0..MEASUREMENT_CACHE {
            system.measure(&TextLayoutRequest::new(format!("Text {index}")));
        }
        assert_eq!(system.measurements.len(), MEASUREMENT_CACHE);
        assert!(system.measurements.iter().all(|(key, _)| *key != first));
        assert_eq!(system.measure(&first), system.shape(&first).measurement);
        assert_eq!(system.measurements.len(), MEASUREMENT_CACHE);
    }

    #[test]
    fn loading_fonts_invalidates_measurements() {
        let mut system = TextSystem::new();
        let mut request = TextLayoutRequest::new("Variable weight");
        request.spans[0].font_family = "Inter".to_string();
        let fallback = system.measure(&request);
        let generation = system.font_generation();

        system.load_font_data(include_bytes!("../assets/inter/InterVariable-Latin.ttf").to_vec());

        assert_eq!(system.font_generation(), generation + 1);
        assert!(system.measurements.is_empty());
        let loaded = system.measure(&request);
        assert_ne!(loaded.width, fallback.width);
        assert_eq!(loaded, system.shape(&request).measurement);
        assert_eq!(system.measure(&request), loaded);
    }

    #[test]
    fn wraps_text_to_max_width() {
        let mut system = TextSystem::new();
        let mut unwrapped = TextLayoutRequest::new("one two three four five");
        unwrapped.spans[0].font_size = 24.0;
        let mut wrapped = unwrapped.clone();
        wrapped.max_width = Some(80.0);

        let wide = system.measure(&unwrapped);
        let narrow = system.measure(&wrapped);

        assert!(wide.width > narrow.width);
        assert!(narrow.height > wide.height);
    }

    #[test]
    fn atlas_contains_glyph_pixels() {
        let mut system = TextSystem::new();
        let mut request = TextLayoutRequest::new("A");
        request.spans[0].font_size = 48.0;
        let layout = system.layout(&request);
        let render = system.render_atlas(&layout, TextRenderMode::Raster, 128);

        assert_eq!(render.instances.len(), 1);
        let instance = render.instances[0];
        assert!(instance.rect[2] > 0.0 && instance.uv_rect[2] > instance.uv_rect[0]);
        assert!(render.pixels.chunks_exact(4).any(|pixel| pixel[3] > 0));
    }

    #[test]
    fn colour_glyphs_are_stored_premultiplied() {
        let mut image = SwashImage::new();
        image.content = SwashContent::Color;
        image.placement.width = 2;
        image.placement.height = 1;
        image.data = vec![255, 0, 0, 128, 0, 100, 200, 255];
        let key = TextSystem::new()
            .layout(&TextLayoutRequest::new("A"))
            .glyphs[0]
            .key
            .0;

        image.source = Source::ColorBitmap(swash::scale::StrikeWith::BestFit);
        let bitmap = raster_pixels(&image, key);
        image.source = Source::ColorOutline(0);
        let outline = raster_pixels(&image, key);

        assert_eq!(bitmap.pixels, [128, 0, 0, 128, 0, 100, 200, 255]);
        assert_eq!(outline.pixels, image.data);
        assert_eq!(bitmap.mode, GLYPH_MODE_RASTER_COLOR);
    }

    #[test]
    fn bitmap_glyphs_snap_to_the_nearest_pixel() {
        let mut system = TextSystem::new();
        let shaped = system.shape(&TextLayoutRequest::new("A"));
        let mut image = SwashImage::new();
        image.source = Source::ColorBitmap(swash::scale::StrikeWith::BestFit);
        image.content = SwashContent::Color;
        image.placement = swash::zeno::Placement {
            left: 1,
            top: 8,
            width: 2,
            height: 2,
        };
        image.data = vec![0; 2 * 2 * 4];
        let mut atlas = GlyphAtlas::new(64);
        let (slot, origin) = atlas.allocate([2, 2]).unwrap();
        for start in [20.0, -21.0] {
            let mut previous = f32::NEG_INFINITY;
            for step in 0..=8 {
                let y = start + step as f32 * 0.125;
                let layout = shaped.place([3.3, y], [1.0; 4]);
                let glyph = &layout.glyphs[0];
                let pixels = raster_pixels(&image, glyph.key.0);
                let placed = PlacedGlyph {
                    slot,
                    origin,
                    size: pixels.size,
                    left: pixels.left,
                    top: pixels.top,
                    mode: pixels.mode,
                };
                let rendered_y =
                    placed.instance(glyph, AtlasKey::Raster(glyph.key.0)).rect[1] + 8.0;
                let pen_y = glyph.position[1];
                // Half a pixel of snapping plus cosmic-text's quarter-pixel bins.
                assert!(
                    (rendered_y - pen_y).abs() <= 0.625,
                    "pen y {pen_y}, drawn at {rendered_y}"
                );
                assert!(rendered_y >= previous, "pen y {pen_y} moved backwards");
                previous = rendered_y;
            }
        }
    }

    #[test]
    fn atlas_evicts_before_running_out_of_allocator_ids() {
        let mut system = TextSystem::new();
        let template = system.layout(&TextLayoutRequest::new("A")).glyphs[0].key.0;
        let mut atlas = GlyphAtlas::new(512);
        for index in 0..70_000_u32 {
            if index % 1000 == 0 {
                atlas.begin_frame();
            }
            let (slot, origin) = atlas.allocate([1, 1]).unwrap();
            let key = CacheKey {
                font_size_bits: index,
                ..template
            };
            atlas.insert(
                AtlasKey::Raster(key),
                AtlasImage::Placed(PlacedGlyph {
                    slot,
                    origin,
                    size: [1, 1],
                    left: 0.0,
                    top: 0.0,
                    mode: GLYPH_MODE_RASTER_MASK,
                }),
            );
        }
        assert_eq!(atlas.len(), MAX_ATLAS_GLYPHS);
    }

    #[test]
    fn raster_glyphs_are_placed_on_whole_pixels() {
        let mut system = TextSystem::new();
        let mut request = TextLayoutRequest::new("AV Sharp text\nمرحبا بالعالم wraps here");
        request.spans[0].font_size = 31.0;
        request.max_width = Some(220.0);
        request.align = TextAlign::Center;
        let shaped = system.shape(&request);

        let first = shaped.place([10.1, 5.3], [1.0; 4]);
        let second = shaped.place([10.6, 5.3], [1.0; 4]);
        let render = system.render_atlas(&second, TextRenderMode::Raster, 1024);

        assert_ne!(first.glyphs[0].key, second.glyphs[0].key);
        assert!(
            first
                .glyphs
                .iter()
                .chain(&second.glyphs)
                .all(|glyph| glyph.x.fract() == 0.0 && glyph.y.fract() == 0.0)
        );
        assert!(render.instances.len() > 10);
        assert!(
            render
                .instances
                .iter()
                .all(|instance| instance.rect[0].fract() == 0.0 && instance.rect[1].fract() == 0.0)
        );
    }

    #[test]
    fn fractional_y_changes_raster_subpixel_bin_and_pixels() {
        let mut system = TextSystem::new();
        let shaped = system.shape(&TextLayoutRequest::new("A"));
        let first = shaped.place([0.0, 0.0], [1.0; 4]);
        let second = shaped.place([0.0, 0.5], [1.0; 4]);

        assert_ne!(first.glyphs[0].key.0.y_bin, second.glyphs[0].key.0.y_bin);
        let first = system.render_atlas(&first, TextRenderMode::Raster, 64);
        let second = system.render_atlas(&second, TextRenderMode::Raster, 64);
        assert_ne!(first.pixels, second.pixels);
    }

    #[test]
    fn raster_ink_follows_fractional_y() {
        let mut system = TextSystem::new();
        let mut request = TextLayoutRequest::new("H");
        request.spans[0].font_size = 24.0;
        let shaped = system.shape(&request);
        let size = 64;
        let ink_center = |system: &mut TextSystem, y: f32| {
            let layout = shaped.place([4.0, y], [1.0; 4]);
            let render = system.render_atlas(&layout, TextRenderMode::Raster, size);
            let instance = render.instances[0];
            let [left, top] = [instance.uv_rect[0] as usize, instance.uv_rect[1] as usize];
            let (mut weight, mut sum) = (0.0, 0.0);
            for row in 0..instance.rect[3] as usize {
                for col in 0..instance.rect[2] as usize {
                    let index = ((top + row) * size as usize + left + col) * 4 + 3;
                    let alpha = f32::from(render.pixels[index]);
                    weight += alpha;
                    sum += alpha * (instance.rect[1] + row as f32 + 0.5);
                }
            }
            sum / weight
        };

        let base = ink_center(&mut system, 20.0);
        for step in [0.25, 0.5, 0.75, 1.0, -0.25, -0.75] {
            let moved = ink_center(&mut system, 20.0 + step) - base;
            assert!(
                (moved - step).abs() < 0.15,
                "origin moved {step}px, ink moved {moved}px"
            );
        }
    }

    #[test]
    fn overflowed_glyph_recovers_when_working_set_shrinks() {
        let mut system = TextSystem::new();
        let mut atlas = GlyphAtlas::new(64);
        let mut overfull = None;
        let mut request = TextLayoutRequest::new("ABCDEFGHIJKLMNOPQRSTUVWXYZ");
        request.spans[0].font_size = 40.0;
        let mut layout = system.layout(&request);
        atlas.begin_frame();
        let full =
            system.prepare_glyphs(&mut atlas, &layout, TextRenderMode::Raster, &mut overfull);
        atlas.commit(full.frame);
        assert!(full.dropped_glyphs > 0);
        let dropped = layout
            .glyphs
            .iter()
            .find(|glyph| !atlas.glyphs.contains_key(&AtlasKey::Raster(glyph.key.0)))
            .unwrap()
            .clone();
        layout.glyphs = vec![dropped];

        atlas.begin_frame();
        let recovered =
            system.prepare_glyphs(&mut atlas, &layout, TextRenderMode::Raster, &mut overfull);
        assert_eq!(recovered.dropped_glyphs, 0);
        assert!(!recovered.instances.is_empty());
        assert!(!recovered.uploads.is_empty());
    }

    #[test]
    fn overflowed_layout_recovers_when_other_layout_becomes_stale() {
        let mut system = TextSystem::new();
        let mut atlas = GlyphAtlas::new(64);
        let mut first = TextLayoutRequest::new("W");
        first.spans[0].font_size = 64.0;
        let first = system.layout(&first);
        let mut second = TextLayoutRequest::new("A");
        second.spans[0].font_size = 40.0;
        let second = system.layout(&second);
        let mut overfull = None;
        atlas.begin_frame();
        let placed = prepare(&mut system, &mut atlas, &first, TextRenderMode::Raster);
        assert!(!placed.instances.is_empty());
        let blocked =
            system.prepare_glyphs(&mut atlas, &second, TextRenderMode::Raster, &mut overfull);
        assert!(blocked.dropped_glyphs > 0);
        atlas.commit(blocked.frame);
        atlas.begin_frame();
        let suppressed =
            system.prepare_glyphs(&mut atlas, &second, TextRenderMode::Raster, &mut overfull);
        assert!(suppressed.uploads.is_empty());
        atlas.begin_frame();
        let recovered =
            system.prepare_glyphs(&mut atlas, &second, TextRenderMode::Raster, &mut overfull);
        assert_eq!(recovered.dropped_glyphs, 0);
        assert!(!recovered.uploads.is_empty());
    }

    #[test]
    fn letter_spacing_widens_advance_and_line_height_spaces_lines() {
        let mut system = TextSystem::new();
        let mut request = TextLayoutRequest::new("Spacing");
        request.spans[0].font_size = 40.0;
        let base = system.layout(&request);
        let glyph_x = |layout: &TextLayout, index: usize| layout.glyphs[index].position[0];

        request.spans[0].letter_spacing = 6.0;
        let tracked = system.layout(&request);
        let count = base.glyphs.len();
        assert!(
            (tracked.measurement.width - base.measurement.width - 6.0 * (count - 1) as f32).abs()
                < 0.01
        );
        assert!(
            (glyph_x(&tracked, count - 1) - glyph_x(&base, count - 1) - 6.0 * (count - 1) as f32)
                .abs()
                < 0.01
        );

        request.spans[0].letter_spacing = -2.0;
        let tight = system.layout(&request);
        assert!(tight.measurement.width < base.measurement.width);

        let line_gap = |system: &mut TextSystem, line_height| {
            let mut request = TextLayoutRequest::new("a\nb");
            request.spans[0].font_size = 40.0;
            request.line_height = line_height;
            let layout = system.layout(&request);
            layout.glyphs[1].position[1] - layout.glyphs[0].position[1]
        };
        assert!((line_gap(&mut system, None) - 48.0).abs() < 0.01);
        assert!((line_gap(&mut system, Some(80.0)) - 80.0).abs() < 0.01);
    }

    #[test]
    fn trailing_letter_spacing_does_not_shift_alignment() {
        let mut system = TextSystem::new();
        let h_x = |system: &mut TextSystem, align, spacing| {
            let mut request = TextLayoutRequest::new("H");
            request.spans[0].font_size = 40.0;
            request.max_width = Some(200.0);
            request.align = align;
            request.spans[0].letter_spacing = spacing;
            system.layout(&request).glyphs[0].position[0]
        };
        for align in [TextAlign::Left, TextAlign::Center, TextAlign::Right] {
            let base = h_x(&mut system, align, 0.0);
            for spacing in [20.0, -10.0] {
                assert!((h_x(&mut system, align, spacing) - base).abs() < 0.01);
            }
        }
    }

    #[test]
    fn letter_spacing_wraps_on_trimmed_line_width() {
        let mut system = TextSystem::new();
        let mut request = TextLayoutRequest::new("HH");
        request.spans[0].font_size = 40.0;
        request.spans[0].letter_spacing = 20.0;
        let natural = system.measure(&request).width;
        let line_count = |system: &mut TextSystem, request: &mut TextLayoutRequest, width| {
            request.max_width = Some(width);
            let layout = system.layout(request);
            let mut ys: Vec<_> = layout
                .glyphs
                .iter()
                .map(|glyph| glyph.position[1])
                .collect();
            ys.dedup_by(|a, b| (*a - *b).abs() < 0.01);
            ys.len()
        };

        assert_eq!(line_count(&mut system, &mut request, natural + 0.5), 1);
        assert_eq!(line_count(&mut system, &mut request, natural - 0.5), 2);

        request.spans[0].letter_spacing = -10.0;
        let natural = system.measure(&request).width;
        assert_eq!(line_count(&mut system, &mut request, natural + 0.5), 1);
        assert_eq!(line_count(&mut system, &mut request, natural - 0.5), 2);
    }

    #[test]
    fn measured_width_excludes_trailing_letter_spacing() {
        let mut system = TextSystem::new();
        let mut request = TextLayoutRequest::new("H");
        request.spans[0].font_size = 40.0;
        let base = system.measure(&request).width;
        for spacing in [20.0, -10.0] {
            request.spans[0].letter_spacing = spacing;
            let shaped = system.shape(&request);
            assert!((shaped.measurement.width - base).abs() < 0.01);
            assert!((shaped.bounds[2] - base).abs() < 0.01);
        }
    }

    #[test]
    fn variable_weight_is_not_quantised_to_hundreds() {
        let mut system = TextSystem::new();
        system.load_font_data(include_bytes!("../assets/inter/InterVariable-Latin.ttf").to_vec());
        let width = |system: &mut TextSystem, weight| {
            let mut request = TextLayoutRequest::new("Variable weight");
            request.spans[0].font_family = "Inter".to_string();
            request.spans[0].font_size = 64.0;
            request.spans[0].font_weight = weight;
            system.measure(&request).width
        };
        let (light, between, regular) = (
            width(&mut system, 400),
            width(&mut system, 450),
            width(&mut system, 500),
        );
        assert!(
            light < between && between < regular,
            "{light} {between} {regular}"
        );
    }

    #[test]
    fn replacing_one_glyph_in_full_atlas_evicts_only_stale_glyph() {
        let mut system = TextSystem::new();
        let mut request = TextLayoutRequest::new("ABCDE");
        request.spans[0].font_size = 24.0;
        let mut layout = system.layout(&request);
        let mut atlas = GlyphAtlas::new(64);
        atlas.begin_frame();
        let mut origins = Vec::new();
        for glyph in &layout.glyphs[..4] {
            let (slot, origin) = atlas.allocate([32, 32]).unwrap();
            let placed = PlacedGlyph {
                slot,
                origin,
                size: [32, 32],
                left: 0.0,
                top: 0.0,
                mode: GLYPH_MODE_RASTER_MASK,
            };
            atlas.insert(AtlasKey::Raster(glyph.key.0), AtlasImage::Placed(placed));
            origins.push(origin);
        }
        atlas.commit(atlas.frame);
        assert!(atlas.allocate([1, 1]).is_none());
        let replaced_key = AtlasKey::Raster(layout.glyphs.remove(3).key.0);

        atlas.begin_frame();
        let replaced = prepare(&mut system, &mut atlas, &layout, TextRenderMode::Raster);
        assert_eq!(replaced.dropped_glyphs, 0);
        assert!(!replaced.compacted);
        assert!(!atlas.glyphs.contains_key(&replaced_key));
        for (glyph, origin) in layout.glyphs.iter().zip(origins).take(3) {
            let AtlasImage::Placed(placed) = atlas.glyphs[&AtlasKey::Raster(glyph.key.0)].image
            else {
                panic!("glyph should stay placed");
            };
            assert_eq!(placed.origin, origin);
        }
        assert!(!replaced.uploads.is_empty());
    }

    #[test]
    fn font_size_sweep_keeps_raster_memory_bounded() {
        let mut system = TextSystem::new();
        let mut atlas = GlyphAtlas::new(2048);
        let mut seen = HashSet::new();
        for frame in 0..60 {
            let mut request = TextLayoutRequest::new("Lumen text");
            request.spans[0].font_size = 32.0 + frame as f32 * (568.0 / 59.0);
            let layout = system.layout(&request);
            seen.extend(layout.glyphs.iter().map(|glyph| glyph.key));
            atlas.begin_frame();
            let prepared = prepare(&mut system, &mut atlas, &layout, TextRenderMode::Raster);
            atlas.commit(prepared.frame);
            assert_eq!(prepared.dropped_glyphs, 0, "frame {frame}");
        }

        assert!(atlas.len() < seen.len() / 2);
        assert!(system.swash_cache.image_cache.is_empty());
    }

    #[test]
    fn uncommitted_uploads_are_repeated() {
        let mut system = TextSystem::new();
        let mut atlas = GlyphAtlas::new(256);
        let layout = system.layout(&TextLayoutRequest::new("Retry"));

        atlas.begin_frame();
        let dropped = prepare(&mut system, &mut atlas, &layout, TextRenderMode::Raster);
        atlas.begin_frame();
        let retried = prepare(&mut system, &mut atlas, &layout, TextRenderMode::Raster);
        atlas.commit(retried.frame);
        atlas.begin_frame();
        let committed = prepare(&mut system, &mut atlas, &layout, TextRenderMode::Raster);

        assert!(!dropped.uploads.is_empty());
        assert_eq!(retried.uploads.len(), dropped.uploads.len());
        assert_eq!(retried.uploads[0].origin, dropped.uploads[0].origin);
        assert!(committed.uploads.is_empty());
    }

    #[test]
    fn whitespace_draws_no_instances_at_any_subpixel_bin() {
        let mut system = TextSystem::new();
        let mut request = TextLayoutRequest::new(format!("{}ab", " ".repeat(200)));
        request.spans[0].font_size = 8.0;
        let layout = system.layout(&request);
        let mut atlas = GlyphAtlas::new(1024);
        atlas.begin_frame();
        let prepared = prepare(&mut system, &mut atlas, &layout, TextRenderMode::Raster);
        assert_eq!(prepared.instances.len(), 2);
    }

    #[test]
    fn layouts_share_the_atlas_within_a_frame() {
        let mut system = TextSystem::new();
        let mut atlas = GlyphAtlas::new(1024);
        let unique: String = ('!'..='~')
            .chain('¡'..='ſ')
            .chain('Α'..='Ω')
            .chain('α'..='ω')
            .chain('А'..='я')
            .collect();
        assert!(unique.chars().count() > 400);
        let split = unique
            .char_indices()
            .nth(unique.chars().count() / 2)
            .unwrap()
            .0;
        let (first, second) = unique.split_at(split);
        let layout = |system: &mut TextSystem, content: &str| {
            let mut request = TextLayoutRequest::new(content);
            request.spans[0].font_size = 28.0;
            request.max_width = Some(900.0);
            system.layout(&request)
        };
        let first = layout(&mut system, first);
        let second = layout(&mut system, second);

        for mode in [TextRenderMode::Raster, TextRenderMode::Msdf] {
            let frame = atlas.begin_frame();
            let a = prepare(&mut system, &mut atlas, &first, mode);
            let b = prepare(&mut system, &mut atlas, &second, mode);
            atlas.commit(frame);

            assert_eq!((a.dropped_glyphs, b.dropped_glyphs), (0, 0), "{mode:?}");
            assert!(!a.compacted && !b.compacted, "{mode:?}");
            let first_glyphs = atlas.keys_for(&first.glyphs, mode, &[]);
            assert!(
                first_glyphs
                    .iter()
                    .all(|key| atlas.glyphs.contains_key(key))
            );
        }
        assert!(atlas.size() <= 1024);
    }

    #[cfg(feature = "msdf")]
    mod msdf_tests {
        use super::*;
        use crate::msdf;

        const COLR_FONT: &[u8] = include_bytes!("../assets/roboto/RobotoColrTest.ttf");

        fn msdf_instances(prepared: &PreparedGlyphs) -> usize {
            prepared
                .instances
                .iter()
                .filter(|instance| instance.render_mode() == GLYPH_MODE_MSDF)
                .count()
        }

        #[test]
        fn small_raster_fallbacks_share_the_msdf_baseline() {
            let mut system = TextSystem::new();
            let mut request = TextLayoutRequest::new("SMALL SIZES AT FRACTIONAL POSITIONS");
            request.spans[0].font_size = 14.0;
            let shaped = system.shape(&request);
            let mut atlas = GlyphAtlas::new(1024);
            for y in [24.0, 24.6, -24.6] {
                let layout = shaped.place([40.3, y], [1.0; 4]);
                atlas.begin_frame();
                // Exercise mixed rendering without depending on system fonts' overlaps.
                atlas.insert(
                    AtlasKey::Msdf(msdf::msdf_key(layout.glyphs[0].key.0)),
                    AtlasImage::RasterOnly,
                );
                let prepared = prepare(&mut system, &mut atlas, &layout, TextRenderMode::Msdf);
                assert!(msdf_instances(&prepared) > 0);
                assert!(msdf_instances(&prepared) < prepared.instances.len());
                let visible = layout.glyphs.iter().filter_map(|glyph| {
                    let key = atlas.key_for(glyph.key, TextRenderMode::Msdf);
                    match atlas.glyphs[&key].image {
                        AtlasImage::Placed(placed) => Some((glyph, placed)),
                        _ => None,
                    }
                });
                for ((glyph, placed), instance) in visible.zip(&prepared.instances) {
                    let baseline = if instance.render_mode() == GLYPH_MODE_MSDF {
                        instance.rect[1]
                            + (placed.top - 0.5) * request.spans[0].font_size
                                / msdf_em_size(request.spans[0].font_size)
                    } else {
                        // Swash's raster offset is in outline coordinates, where Y points up.
                        instance.rect[1] + placed.top - glyph.key.0.y_bin.as_float()
                    };
                    assert!(
                        (baseline - glyph.position[1]).abs() < 0.125,
                        "glyph {:?}: rendered baseline {baseline}, layout baseline {}",
                        glyph.key,
                        glyph.position[1],
                    );
                }
            }
        }

        #[test]
        fn font_size_sweep_generates_each_field_once() {
            let mut system = TextSystem::new();
            let mut atlas = GlyphAtlas::new(4096);
            let mut uploads = 0;
            let mut fields = HashSet::new();
            for frame in 0..40 {
                let mut request = TextLayoutRequest::new("Lumen MSDF text");
                request.spans[0].font_size = 32.0 + frame as f32 * (968.0 / 39.0);
                request.origin = [frame as f32 * 0.37, 0.0];
                let layout = system.layout(&request);
                fields.extend(
                    layout
                        .glyphs
                        .iter()
                        .map(|glyph| msdf::msdf_key(glyph.key.0)),
                );
                atlas.begin_frame();
                let prepared = prepare(&mut system, &mut atlas, &layout, TextRenderMode::Msdf);
                atlas.commit(prepared.frame);

                assert_eq!(prepared.dropped_glyphs, 0, "frame {frame}");
                assert!(msdf_instances(&prepared) > 0);
                uploads += prepared.uploads.iter().filter(|upload| {
                    atlas.glyphs.iter().any(|(key, glyph)| {
                        matches!(key, AtlasKey::Msdf(_)) && matches!(glyph.image, AtlasImage::Placed(placed) if placed.origin == upload.origin)
                    })
                }).count();
            }

            assert!(uploads <= fields.len());
            assert_eq!(atlas.size(), 4096);
        }

        #[test]
        fn field_resolution_grows_with_font_size() {
            assert_eq!(msdf_em_size(14.0), MSDF_EM_SIZE);
            assert_eq!(msdf_em_size(4.0 * MSDF_EM_SIZE), MSDF_EM_SIZE);
            assert_eq!(msdf_em_size(4.0 * MSDF_EM_SIZE + 1.0), 2.0 * MSDF_EM_SIZE);
            assert_eq!(msdf_em_size(1000.0), MSDF_MAX_EM_SIZE);
            assert_eq!(msdf_em_size(100_000.0), MSDF_MAX_EM_SIZE);
        }

        #[test]
        fn oversized_raster_glyphs_fall_back_to_distance_fields() {
            let mut system = TextSystem::new();
            let mut request = TextLayoutRequest::new("H");
            request.spans[0].font_size = 1500.0;
            let layout = system.layout(&request);
            let mut atlas = GlyphAtlas::new(512);
            atlas.begin_frame();
            let raster = prepare(&mut system, &mut atlas, &layout, TextRenderMode::Raster);
            assert_eq!(raster.dropped_glyphs, 0);
            assert_eq!(msdf_instances(&raster), 1);
            atlas.commit(raster.frame);

            // The oversized raster key is remembered, so the next frame does not
            // rasterize it again.
            atlas.begin_frame();
            let again = prepare(&mut system, &mut atlas, &layout, TextRenderMode::Raster);
            assert_eq!(msdf_instances(&again), 1);
            assert!(again.uploads.is_empty());

            // Colour glyphs have no outline to scale, so they are still dropped.
            system.load_font_data(COLR_FONT.to_vec());
            let mut colour = TextLayoutRequest::new("A");
            colour.spans[0].font_family = "Roboto COLR Test".to_string();
            colour.spans[0].font_size = 1500.0;
            let colour = system.layout(&colour);
            atlas.begin_frame();
            let colour = prepare(&mut system, &mut atlas, &colour, TextRenderMode::Raster);
            assert_eq!(colour.instances.len(), 0);
            assert_eq!(colour.dropped_glyphs, 1);
        }

        #[test]
        fn colour_glyphs_are_not_stroked() {
            let mut system = TextSystem::new();
            system.load_font_data(COLR_FONT.to_vec());
            let mut colour = TextLayoutRequest::new("AA");
            colour.spans[0].font_family = "Roboto COLR Test".to_string();
            let colour = system.layout(&colour);
            let mut atlas = GlyphAtlas::new(1024);
            atlas.begin_frame();
            let colour = system.prepare_stroked_glyphs(
                &mut atlas,
                &colour,
                TextRenderMode::Raster,
                &[1.0],
                &mut None,
            );
            assert_eq!(colour.instances.len(), 2);
            assert_eq!((colour.strokes.len(), colour.dropped_glyphs), (0, 0));
        }

        #[test]
        fn wide_strokes_widen_the_field_range_until_it_outgrows_the_atlas() {
            let mut system = TextSystem::new();
            let mut request = TextLayoutRequest::new("I");
            request.spans[0].font_size = 192.0;
            let layout = system.layout(&request);
            let mut atlas = GlyphAtlas::new(256);
            for mode in [TextRenderMode::Msdf, TextRenderMode::Raster] {
                atlas.begin_frame();
                let prepared =
                    system.prepare_stroked_glyphs(&mut atlas, &layout, mode, &[64.0], &mut None);
                atlas.commit(prepared.frame);
                let [stroke] = prepared.strokes[..] else {
                    panic!("{mode:?}: expected one stroke");
                };
                assert_eq!(stroke.render_mode(), GLYPH_MODE_MSDF, "{mode:?}");
                // 48 texels per em, where the stroke reaches 16 texels.
                let tier = (stroke.flags >> GLYPH_RANGE_SHIFT) & GLYPH_RANGE_MASK;
                assert!(
                    MSDF_PX_RANGE * (1 << tier) as f32 - MSDF_PX_RANGE / 2.0 - 1.5 >= 16.0,
                    "{mode:?}"
                );
            }
        }

        #[test]
        fn fields_too_large_for_the_atlas_are_dropped_before_they_are_generated() {
            let mut system = TextSystem::new();
            let mut request = TextLayoutRequest::new("I");
            request.spans[0].font_size = 48.0;
            let layout = system.layout(&request);
            let mut atlas = GlyphAtlas::new(128);
            let mode = TextRenderMode::Msdf;
            for half_width in [1000.0, 7000.0] {
                let start = std::time::Instant::now();
                atlas.begin_frame();
                let prepared = system.prepare_stroked_glyphs(
                    &mut atlas,
                    &layout,
                    mode,
                    &[half_width],
                    &mut None,
                );
                assert_eq!(prepared.instances.len(), 1, "{half_width}");
                assert_eq!(
                    (prepared.strokes.len(), prepared.dropped_glyphs),
                    (0, 1),
                    "{half_width}"
                );
                assert!(
                    start.elapsed() < std::time::Duration::from_secs(1),
                    "{half_width}"
                );
            }
        }

        #[test]
        fn unused_field_markers_are_forgotten() {
            let mut system = TextSystem::new();
            let mut atlas = GlyphAtlas::new(256);
            let mut prepare_text = |atlas: &mut GlyphAtlas, text: &str| {
                let layout = system.layout(&TextLayoutRequest::new(text));
                atlas.begin_frame();
                prepare(&mut system, atlas, &layout, TextRenderMode::Msdf);
            };
            prepare_text(&mut atlas, "A B");
            assert!(atlas.len() > 2);
            prepare_text(&mut atlas, "A");
            prepare_text(&mut atlas, "A");
            assert!(
                atlas
                    .glyphs
                    .values()
                    .all(|glyph| matches!(glyph.image, AtlasImage::Placed(_)))
            );
        }

        #[test]
        fn animated_widths_keep_an_overfull_field_working_set() {
            let mut system = TextSystem::new();
            let mut request = TextLayoutRequest::new("IOH");
            request.spans[0].font_size = 48.0;
            let layout = system.layout(&request);
            let mut atlas = GlyphAtlas::new(64);
            let mut overfull = None;
            for (frame, half_width) in [0.25, 0.25, 0.4, 0.7, 0.9].into_iter().enumerate() {
                atlas.begin_frame();
                let prepared = system.prepare_stroked_glyphs(
                    &mut atlas,
                    &layout,
                    TextRenderMode::Msdf,
                    &[half_width],
                    &mut overfull,
                );
                atlas.commit(prepared.frame);
                assert!(prepared.dropped_glyphs > 0, "frame {frame}");
                if frame > 0 {
                    assert!(!prepared.compacted, "frame {frame} compacted");
                    assert!(prepared.uploads.is_empty(), "frame {frame} uploaded");
                }
            }
        }

        #[test]
        fn moving_the_origin_keeps_an_overfull_field_working_set() {
            let mut system = TextSystem::new();
            let mut request = TextLayoutRequest::new("IOH");
            request.spans[0].font_size = 48.0;
            let shaped = system.shape(&request);
            let mut atlas = GlyphAtlas::new(64);
            let mut overfull = None;
            for (frame, origin) in [0.0, 0.0, 0.5].into_iter().enumerate() {
                let layout = shaped.place([origin, 0.0], [1.0; 4]);
                atlas.begin_frame();
                let prepared = system.prepare_stroked_glyphs(
                    &mut atlas,
                    &layout,
                    TextRenderMode::Msdf,
                    &[0.25],
                    &mut overfull,
                );
                atlas.commit(prepared.frame);
                assert!(prepared.dropped_glyphs > 0, "frame {frame}");
                // Fields do not depend on the subpixel position, so the working set is the same.
                if frame > 0 {
                    assert!(prepared.uploads.is_empty(), "frame {frame} uploaded");
                }
            }
        }

        #[test]
        fn oversized_fallbacks_stay_stable_when_the_atlas_overflows() {
            let mut system = TextSystem::new();
            let mut request = TextLayoutRequest::new("HWI");
            request.spans[0].font_size = 1500.0;
            let layout = system.layout(&request);
            let mut atlas = GlyphAtlas::new(512);
            let mut overfull = None;
            let mut frames = Vec::new();
            for _ in 0..6 {
                atlas.begin_frame();
                let prepared = system.prepare_glyphs(
                    &mut atlas,
                    &layout,
                    TextRenderMode::Raster,
                    &mut overfull,
                );
                atlas.commit(prepared.frame);
                frames.push((prepared.instances.len(), prepared.uploads.len()));
            }
            let (instances, _) = frames[0];
            assert!(instances > 0);
            for (frame, (count, uploads)) in frames.iter().enumerate() {
                assert_eq!(*count, instances, "frame {frame}");
                assert!(frame == 0 || *uploads == 0, "frame {frame} uploaded");
            }
        }

        #[test]
        fn auto_mode_uses_fields_from_the_size_threshold() {
            let mut system = TextSystem::new();
            let mut atlas = GlyphAtlas::new(2048);
            for (size, fields) in [
                (MSDF_AUTO_MIN_SIZE - 1.0, 0),
                (MSDF_AUTO_MIN_SIZE, 1),
                (MSDF_AUTO_MIN_SIZE + 100.0, 1),
            ] {
                let mut request = TextLayoutRequest::new("H");
                request.spans[0].font_size = size;
                let layout = system.layout(&request);
                atlas.begin_frame();
                let prepared = prepare(&mut system, &mut atlas, &layout, TextRenderMode::Auto);
                assert_eq!(prepared.dropped_glyphs, 0);
                assert_eq!(msdf_instances(&prepared), fields, "size {size}");
                assert_eq!(prepared.instances.len(), 1);
            }
        }

        /// Pixels of `window` (`[x, y, width, height]`) covered by one glyph, as the
        /// text shader computes them: raster glyphs read 1:1, fields are
        /// bilinearly filtered and sharpened.
        fn glyph_coverage(
            system: &mut TextSystem,
            layout: &TextLayout,
            mode: TextRenderMode,
            window: [i32; 4],
        ) -> Vec<f32> {
            const ATLAS: u32 = 4096;
            let render = system.render_atlas(layout, mode, ATLAS);
            let [instance] = render.instances[..] else {
                panic!("expected one glyph");
            };
            let [x, y, width, height] = instance.rect;
            let [origin_x, origin_y, window_width, window_height] = window;
            let texel = |x: i32, y: i32| -> [f32; 4] {
                let at = (y as usize * ATLAS as usize + x as usize) * 4;
                std::array::from_fn(|i| f32::from(render.pixels[at + i]) / 255.0)
            };
            let [u0, v0, u1, _] = instance.uv_rect;
            let texels_per_pixel = (u1 - u0) / width;
            let screen_px_range = (0.5 * MSDF_PX_RANGE / texels_per_pixel).max(1.0);
            let mut coverage = Vec::new();
            for py in 0..window_height {
                for px in 0..window_width {
                    let (fx, fy) = (
                        (origin_x + px) as f32 + 0.5 - x,
                        (origin_y + py) as f32 + 0.5 - y,
                    );
                    let (inside_x, inside_y) =
                        ((0.0..width).contains(&fx), (0.0..height).contains(&fy));
                    coverage.push(if !(inside_x && inside_y) {
                        0.0
                    } else if instance.render_mode() == GLYPH_MODE_MSDF {
                        let (u, v) = (
                            u0 + fx * texels_per_pixel - 0.5,
                            v0 + fy * texels_per_pixel - 0.5,
                        );
                        let (tx, ty) = (u.floor(), v.floor());
                        let (wx, wy) = (u - tx, v - ty);
                        let mut median = [0.0; 3];
                        for (channel, value) in median.iter_mut().enumerate() {
                            let at =
                                |dx: i32, dy: i32| texel(tx as i32 + dx, ty as i32 + dy)[channel];
                            *value = (at(0, 0) * (1.0 - wx) + at(1, 0) * wx) * (1.0 - wy)
                                + (at(0, 1) * (1.0 - wx) + at(1, 1) * wx) * wy;
                        }
                        let [r, g, b] = median;
                        let median = r.min(g).max(r.max(g).min(b));
                        (screen_px_range * (median - 0.5) + 0.5).clamp(0.0, 1.0)
                    } else {
                        texel((u0 + fx) as i32, (v0 + fy) as i32)[3]
                    });
                }
            }
            coverage
        }

        /// Fields drawn at 1000 px must stay within a pixel of the raster outline.
        /// A field with 48 texels per em cuts facets of more than a pixel into the
        /// sharp corners and curves of these glyphs.
        #[test]
        fn large_fields_keep_corners_sharp() {
            let mut system = TextSystem::new();
            for text in ["R", "W", "a", "4"] {
                let mut request = TextLayoutRequest::new(text);
                request.spans[0].font_size = 1000.0;
                request.disable_hinting = true;
                let layout = system.layout(&request);
                let [x, y] = layout.glyphs[0].position.map(|axis| axis.floor() as i32);
                let window = [x - 50, y - 800, 800, 1100];
                let raster = glyph_coverage(&mut system, &layout, TextRenderMode::Raster, window);
                let msdf = glyph_coverage(&mut system, &layout, TextRenderMode::Msdf, window);
                let (width, height) = (window[2] as usize, window[3] as usize);
                let mut outliers = 0;
                for py in 1..height - 1 {
                    for px in 1..width - 1 {
                        let (mut low, mut high) = (1.0_f32, 0.0_f32);
                        for ny in py - 1..=py + 1 {
                            for nx in px - 1..=px + 1 {
                                low = low.min(raster[ny * width + nx]);
                                high = high.max(raster[ny * width + nx]);
                            }
                        }
                        let value = msdf[py * width + px];
                        outliers += usize::from((low - value).max(value - high) > 0.1);
                    }
                }
                assert!(outliers <= 16, "{text}: {outliers} pixels off the outline");
            }
        }

        #[test]
        fn colour_and_overlapping_glyphs_fall_back_to_raster() {
            let mut system = TextSystem::new();
            system.load_font_data(COLR_FONT.to_vec());
            let mut colour = TextLayoutRequest::new("A");
            colour.spans[0].font_family = "Roboto COLR Test".to_string();
            colour.spans[0].font_size = 48.0;
            let colour = system.layout(&colour);

            let mut overlapping = system.layout(&TextLayoutRequest::new("O"));
            let mut atlas = GlyphAtlas::new(512);
            atlas.begin_frame();
            let colour = prepare(&mut system, &mut atlas, &colour, TextRenderMode::Msdf);
            assert_eq!(colour.instances.len(), 1);
            assert_eq!(colour.instances[0].render_mode(), GLYPH_MODE_RASTER_COLOR);

            // Two copies of one outline overlap everywhere.
            let key = msdf::msdf_key(overlapping.glyphs[0].key.0);
            let commands = system
                .swash_cache
                .get_outline_commands_uncached(&mut system.font_system, key)
                .unwrap();
            let overlaps = |commands: &[swash::zeno::Command]| {
                msdf::contours_overlap(&msdf::contours(commands, |p| {
                    fdsm::bezier::Point::new(f64::from(p.x), f64::from(p.y))
                }))
            };
            assert!(!overlaps(&commands));
            assert!(overlaps(&[&commands[..], &commands[..]].concat()));

            overlapping.glyphs[0].key.0.flags |= CacheKeyFlags::FAKE_ITALIC;
            let italic = prepare(&mut system, &mut atlas, &overlapping, TextRenderMode::Msdf);
            assert_eq!(msdf_instances(&italic), 1);
        }
    }

    fn items(system: &mut TextSystem, request: &TextLayoutRequest) -> (Vec<[u32; 3]>, TextItem) {
        let layout = system.layout(request);
        let items = layout
            .glyphs
            .iter()
            .map(|glyph| [glyph.item.glyph, glyph.item.word, glyph.item.line])
            .collect();
        (items, layout.counts)
    }

    #[test]
    fn glyphs_words_and_lines_are_numbered_through_spaces_and_paragraphs() {
        let mut system = TextSystem::new();
        let (items, counts) = items(&mut system, &TextLayoutRequest::new(" ab  cd\n\nef"));
        // Whitespace counts as glyphs and belongs to the word before it, and the empty
        // paragraph is a line of its own.
        assert_eq!(
            items,
            [
                [0, 0, 0],
                [1, 0, 0],
                [2, 0, 0],
                [3, 0, 0],
                [4, 0, 0],
                [5, 1, 0],
                [6, 1, 0],
                [7, 2, 2],
                [8, 2, 2],
            ]
        );
        assert_eq!(
            counts,
            TextItem {
                glyph: 9,
                word: 3,
                line: 3,
                span: 1,
            }
        );
    }

    #[test]
    fn wrapped_words_take_the_next_line() {
        let mut system = TextSystem::new();
        let mut request = TextLayoutRequest::new("one two three");
        request.spans[0].font_size = 20.0;
        request.max_width = Some(50.0);
        let (items, counts) = items(&mut system, &request);
        let words: Vec<_> = items.iter().map(|[_, word, line]| (*word, *line)).collect();
        assert_eq!(&words[..3], [(0, 0); 3]);
        assert_eq!(*words.last().unwrap(), (2, 2));
        assert_eq!((counts.word, counts.line), (3, 3));
    }

    #[test]
    fn wrapping_does_not_change_glyph_or_word_numbering() {
        let mut system = TextSystem::new();
        let mut request = TextLayoutRequest::new("one two three");
        request.spans[0].font_size = 20.0;
        let (unwrapped, unwrapped_counts) = items(&mut system, &request);
        request.max_width = Some(50.0);
        let (wrapped, wrapped_counts) = items(&mut system, &request);
        assert_eq!(unwrapped_counts.glyph, 13);
        assert_eq!(wrapped_counts.glyph, unwrapped_counts.glyph);
        assert_eq!(wrapped_counts.word, unwrapped_counts.word);
        assert_eq!(unwrapped_counts.line, 1);
        assert_eq!(wrapped_counts.line, 3);
        // The wrap drops the two separating spaces from the drawn glyphs, but the glyphs
        // around them keep their numbers.
        let numbering = |items: &[[u32; 3]]| -> Vec<[u32; 2]> {
            items
                .iter()
                .map(|[glyph, word, _]| [*glyph, *word])
                .collect()
        };
        let mut expected = numbering(&unwrapped);
        expected.retain(|[glyph, _]| ![3, 7].contains(glyph));
        assert_eq!(numbering(&wrapped), expected);
    }

    #[test]
    fn marks_and_ligatures_share_one_glyph_index_and_pivot() {
        let mut system = TextSystem::new();
        let mut request = TextLayoutRequest::new("A\u{338}x fix");
        request.spans[0].font_size = 40.0;
        let layout = system.layout(&request);
        let (items, counts) = items(&mut system, &request);
        // The overlay mark is a glyph of its own, numbered and pivoting with its base.
        assert_eq!(items[0], [0, 0, 0]);
        assert_eq!(items[1], [0, 0, 0]);
        assert_eq!(layout.glyphs[0].pivot, layout.glyphs[1].pivot);
        assert_eq!(items[2], [1, 0, 0]);
        // Roboto joins `fi` into one glyph, which is one cluster.
        assert_eq!(items[3..], [[2, 0, 0], [3, 1, 0], [4, 1, 0]]);
        assert_eq!(counts.glyph, 5);
        assert!(layout.glyphs[4].pivot[0] < layout.glyphs[5].pivot[0]);
        // Pivots sit mid-line, at the middle of each cluster's advance.
        let line_middle = 0.6 * 40.0;
        assert!((layout.glyphs[2].pivot[1] - line_middle).abs() < 0.01);
    }

    fn span(text: &str, font_size: f32) -> TextSpan {
        TextSpan {
            font_size,
            ..TextSpan::new(text)
        }
    }

    #[test]
    fn plain_text_measures_as_before_spans() {
        let mut system = TextSystem::new();
        // Recorded from the single-attribute shaping that spans replaced.
        for (spacing, align, measurement, bounds, first, last) in [
            (
                0.0,
                TextAlign::Left,
                [210.57129, 108.0],
                [0.0, 0.0, 210.57129, 108.0],
                [0.0, 28.253906],
                [169.6875, 100.25391],
            ),
            (
                3.0,
                TextAlign::Center,
                [255.57129, 108.0],
                [2.2143555, 0.0, 255.57129, 108.0],
                [2.2143555, 28.253906],
                [224.24658, 100.25391],
            ),
            (
                -1.5,
                TextAlign::Right,
                [188.07129, 108.0],
                [71.92871, 0.0, 188.07129, 108.0],
                [71.92871, 28.253906],
                [245.80566, 100.25391],
            ),
        ] {
            let mut request = TextLayoutRequest::new("Plain text keeps measuring\nthe same way");
            request.spans[0].font_size = 30.0;
            request.spans[0].letter_spacing = spacing;
            request.max_width = Some(260.0);
            request.align = align;
            let shaped = system.shape(&request);
            let layout = shaped.place([0.0, 0.0], [1.0; 4]);
            let close = |a: &[f32], b: &[f32]| a.iter().zip(b).all(|(a, b)| (a - b).abs() < 1e-3);
            let measured = [shaped.measurement.width, shaped.measurement.height];
            assert!(close(&measured, &measurement), "{spacing}: {measured:?}");
            assert!(
                close(&shaped.bounds, &bounds),
                "{spacing}: {:?}",
                shaped.bounds
            );
            assert!(close(&layout.glyphs[0].position, &first));
            assert!(close(&layout.glyphs.last().unwrap().position, &last));
            assert_eq!(layout.glyphs.len(), 37);
            assert_eq!(
                shaped.counts,
                TextItem {
                    glyph: 38,
                    word: 7,
                    line: 3,
                    span: 1,
                }
            );
        }
    }

    #[test]
    fn spans_keep_their_style_and_lines_follow_their_largest_span() {
        let mut system = TextSystem::new();
        let bold = TextSpan {
            font_weight: 700,
            ..span("BIG", 48.0)
        };
        let request =
            TextLayoutRequest::from_spans(vec![span("small ", 16.0), bold, span("\nsmall", 16.0)]);
        let layout = system.layout(&request);

        let spans: Vec<_> = layout.glyphs.iter().map(|glyph| glyph.item.span).collect();
        assert_eq!(spans, [0, 0, 0, 0, 0, 0, 1, 1, 1, 2, 2, 2, 2, 2]);
        assert_eq!(layout.counts.span, 3);
        let size = |glyph: &TextGlyph| f32::from_bits(glyph.key.0.font_size_bits);
        assert_eq!(size(&layout.glyphs[0]), 16.0);
        assert_eq!(size(&layout.glyphs[6]), 48.0);
        assert_eq!(layout.glyphs[6].key.0.font_weight, Weight(700));
        // Both glyph sizes share the first line's baseline, and each line is 1.2 × its
        // largest size.
        assert_eq!(layout.glyphs[0].position[1], layout.glyphs[6].position[1]);
        assert!((layout.measurement.height - (57.6 + 19.2)).abs() < 0.01);
        let lines: Vec<_> = layout.glyphs.iter().map(|glyph| glyph.item.line).collect();
        assert_eq!(lines[9..], [1; 5]);
    }

    #[test]
    fn segments_follow_spans_across_wrapped_lines() {
        let mut system = TextSystem::new();
        let text = "beta gamma delta";
        let mut request =
            TextLayoutRequest::from_spans(vec![span("alpha ", 20.0), span(text, 28.0)]);
        request.max_width = Some(160.0);
        let shaped = system.shape(&request);

        let segments: Vec<_> = shaped
            .segments
            .iter()
            .filter(|segment| segment.span == 1)
            .collect();
        assert!(segments.len() >= 2, "{:?}", shaped.segments);
        assert_eq!(shaped.segments[0].span, 0);
        assert_eq!(shaped.segments[0].x[1], segments[0].x[0]);
        assert_eq!(segments[0].bytes.start, 0);
        assert_eq!(segments.last().unwrap().bytes.end, text.len());
        for pair in segments.windows(2) {
            assert_eq!(pair[1].line, pair[0].line + 1);
            assert!(pair[1].bytes.start > pair[0].bytes.end);
            assert!(pair[1].baseline > pair[0].baseline);
            assert_eq!(pair[1].line_top, pair[0].line_top + pair[0].line_height);
        }
        for segment in &segments {
            assert!(!segment.rtl && segment.x[1] > segment.x[0]);
            assert!(
                segment.ascent > 20.0 && segment.descent > 4.0,
                "{segment:?}"
            );
            assert!(segment.line_top + segment.line_height >= segment.baseline + segment.descent);
        }
    }

    #[test]
    fn letter_spacing_is_per_span_and_trimmed_after_each_line() {
        let mut system = TextSystem::new();
        let tracked = |first: f32, second: f32| {
            TextLayoutRequest::from_spans(vec![
                TextSpan {
                    letter_spacing: first,
                    ..span("HH", 40.0)
                },
                TextSpan {
                    letter_spacing: second,
                    ..span("II", 40.0)
                },
            ])
        };
        let base = system.layout(&tracked(0.0, 0.0));
        let layout = system.layout(&tracked(10.0, 30.0));
        // The last glyph's own spacing is trimmed.
        let expected = base.measurement.width + 10.0 * 2.0 + 30.0;
        assert!((layout.measurement.width - expected).abs() < 0.01);
        assert!((layout.bounds[2] - expected).abs() < 0.01);
        assert!((layout.glyphs[3].position[0] - base.glyphs[3].position[0] - 50.0).abs() < 0.01);

        let mut request = tracked(10.0, 30.0);
        request.max_width = Some(400.0);
        request.align = TextAlign::Right;
        let shaped = system.shape(&request);
        assert!((shaped.bounds[0] + shaped.bounds[2] - 400.0).abs() < 0.01);
        assert!((shaped.segments[1].x[1] - 400.0).abs() < 0.01);
    }

    #[test]
    fn spans_report_decoration_metrics_for_their_size() {
        let mut system = TextSystem::new();
        let shaped = system.shape(&TextLayoutRequest::from_spans(vec![
            span("Under", 40.0),
            span("line", 80.0),
            span("", 80.0),
        ]));
        let [small, large, empty] = shaped.span_metrics[..] else {
            panic!("expected three spans");
        };
        assert!(small.underline_offset > 0.0 && small.underline_thickness > 0.0);
        assert!(small.strikeout_offset < 0.0 && small.strikeout_thickness > 0.0);
        assert!((large.underline_thickness - 2.0 * small.underline_thickness).abs() < 1e-3);
        assert!((large.strikeout_offset - 2.0 * small.strikeout_offset).abs() < 1e-3);
        assert_eq!(empty, TextSpanMetrics::default());
    }

    #[test]
    fn decorations_run_along_spans_and_extents_follow_reading_order() {
        let mut system = TextSystem::new();
        let underline = |spans: Range<usize>, extent: [f32; 2]| TextDecoration {
            shifted: false,
            kind: TextDecorationKind::Underline(TextLineDecoration {
                style: TextLineStyle::Solid,
                thickness: None,
                offset: None,
            }),
            spans,
            extent,
        };
        let rects = |shaped: &ShapedText, decoration: &TextDecoration, paint: fn(usize) -> u32| {
            let mut instances = Vec::new();
            shaped.decorate(decoration, [0.0; 2], paint, &mut instances);
            instances
        };
        let close = |a: f32, b: f32| (a - b).abs() < 0.01;

        // One rect across mixed sizes, as thick and low as the larger span's line.
        let shaped = system.shape(&TextLayoutRequest::from_spans(vec![
            span("Big", 40.0),
            span("small", 20.0),
        ]));
        let [line] = rects(&shaped, &underline(0..2, [0.0, 1.0]), |_| 0)[..] else {
            panic!("expected one rect");
        };
        let big = shaped.span_metrics[0];
        assert!(close(
            line.rect[1],
            shaped.segments[0].baseline + big.underline_offset
        ));
        assert!(close(line.rect[3], big.underline_thickness));
        assert!(close(line.rect[0], 0.0) && close(line.rect[2], shaped.segments[1].x[1]));
        // Paint changes split it into rects that join without antialiasing.
        let [first, second] = rects(&shaped, &underline(0..2, [0.0, 1.0]), |span| span as u32)[..]
        else {
            panic!("expected two rects");
        };
        assert_eq!((first.paint(), second.paint()), (0, 1));
        assert_eq!(first.flags & RECT_JOINED_RIGHT, RECT_JOINED_RIGHT);
        assert_eq!(second.flags & RECT_JOINED_LEFT, RECT_JOINED_LEFT);
        assert!(close(first.rect[0] + first.rect[2], second.rect[0]));
        assert_eq!(
            [first.rect[1], first.rect[3]],
            [second.rect[1], second.rect[3]]
        );

        // The extent runs on across the line break: the quarter to three quarters of the
        // width of both lines together.
        let shaped = system.shape(&TextLayoutRequest::new("abcd\nefgh"));
        let [top, bottom] = rects(&shaped, &underline(0..1, [0.25, 0.75]), |_| 0)[..] else {
            panic!("expected a rect per line");
        };
        let [first, second] = [&shaped.segments[0], &shaped.segments[1]];
        let width = |segment: &TextSegment| segment.x[1] - segment.x[0];
        let total = width(first) + width(second);
        assert!(close(top.rect[0], first.x[0] + total * 0.25));
        assert!(close(top.rect[0] + top.rect[2], first.x[1]));
        assert!(close(bottom.rect[0], second.x[0]));
        assert!(close(bottom.rect[2], total * 0.75 - width(first)));
        assert!(bottom.rect[1] > top.rect[1]);

        // Right-to-left text is drawn in from the right.
        let shaped = system.shape(&TextLayoutRequest::from_spans(vec![span("שלום", 30.0)]));
        let segment = &shaped.segments[0];
        assert!(segment.rtl);
        let [half] = rects(&shaped, &underline(0..1, [0.0, 0.5]), |_| 0)[..] else {
            panic!("expected one rect");
        };
        assert!(close(half.rect[0] + half.rect[2], segment.x[1]));
        assert!(close(half.rect[2], width(segment) * 0.5));

        // A highlight box covers each line's ascent and descent and its padding.
        let shaped = system.shape(&TextLayoutRequest::new("ab\ncd"));
        let highlight = TextDecoration {
            shifted: false,
            kind: TextDecorationKind::Highlight {
                padding: [4.0, 2.0],
                radius: 3.0,
            },
            spans: 0..1,
            extent: [0.0, 1.0],
        };
        let boxes = rects(&shaped, &highlight, |_| 0);
        assert_eq!(boxes.len(), 2);
        for (rect, segment) in boxes.iter().zip(&shaped.segments) {
            assert!(close(rect.rect[0], segment.x[0] - 4.0));
            assert!(close(rect.rect[1], segment.baseline - segment.ascent - 2.0));
            assert!(close(rect.rect[3], segment.ascent + segment.descent + 4.0));
            assert_eq!(
                (rect.uv_rect[0], rect.flags >> RECT_SHAPE_SHIFT),
                (3.0, RECT_BOX)
            );
        }
    }

    #[test]
    fn plain_text_keeps_cr_and_lf_line_endings() {
        let mut system = TextSystem::new();
        let trailing = system.shape(&TextLayoutRequest::new("a\n"));
        assert_eq!(trailing.counts.line, 2);
        assert!((trailing.measurement.height - 38.4).abs() < 0.01);
        let carriage = system.shape(&TextLayoutRequest::new("a\rB"));
        assert_eq!(carriage.counts.line, 2);
        assert_eq!(carriage.place([0.0; 2], [1.0; 4]).glyphs.len(), 2);
        assert_eq!(carriage.segments[1].bytes, 2..3);
    }

    #[test]
    fn a_cluster_crossing_spans_takes_the_first_spans_style() {
        let mut system = TextSystem::new();
        let shaped = system.shape(&TextLayoutRequest::from_spans(vec![
            span("f", 16.0),
            span("i", 80.0),
        ]));
        let spans: Vec<_> = shaped
            .place([0.0; 2], [1.0; 4])
            .glyphs
            .iter()
            .map(|glyph| glyph.item.span)
            .collect();
        // The "fi" ligature is one glyph in the first span, so the second span's size is lost.
        assert_eq!(spans, [0]);
        assert!((shaped.measurement.height - 19.2).abs() < 0.01);
    }

    #[test]
    fn splitting_a_span_does_not_change_its_width() {
        let mut system = TextSystem::new();
        let whole = system.measure(&TextLayoutRequest::from_spans(vec![span("HHHH", 40.0)]));
        let split = system.measure(&TextLayoutRequest::from_spans(vec![
            span("HH", 40.0),
            span("HH", 40.0),
        ]));
        assert_eq!(whole.width, split.width);
    }

    #[test]
    fn justified_right_to_left_text_measures_its_ink() {
        let mut system = TextSystem::new();
        let mut request = TextLayoutRequest::from_spans(vec![span("שלום עולם", 30.0)]);
        request.max_width = Some(260.0);
        request.align = TextAlign::Justified;
        let shaped = system.shape(&request);
        assert!((shaped.measurement.width - 113.83301).abs() < 0.01);
        assert!((shaped.measurement.width - shaped.bounds[2]).abs() < 0.01);
    }

    #[test]
    fn negative_tracking_measures_from_the_line_start() {
        let mut system = TextSystem::new();
        let mut request = TextLayoutRequest::from_spans(vec![TextSpan {
            letter_spacing: -10.0,
            ..span(" H", 30.0)
        }]);
        request.max_width = Some(260.0);
        request.align = TextAlign::Right;
        let shaped = system.shape(&request);
        assert!((shaped.measurement.width - 18.813477).abs() < 0.01);
        assert!((shaped.bounds[0] + shaped.bounds[2] - 260.0).abs() < 0.01);
    }

    #[test]
    fn justified_lines_with_moved_glyphs_measure_the_full_width() {
        let mut system = TextSystem::new();
        let mut request = TextLayoutRequest::from_spans(vec![
            TextSpan {
                letter_spacing: -10.0,
                ..span("  H ", 30.0)
            },
            span("H H H H H H H H H H", 30.0),
        ]);
        request.max_width = Some(260.0);
        request.align = TextAlign::Justified;
        let shaped = system.shape(&request);
        assert!((shaped.measurement.width - 260.0).abs() < 0.01);
    }

    #[test]
    fn bidi_text_with_negative_tracking_measures_as_before() {
        let mut system = TextSystem::new();
        let mut request = TextLayoutRequest::from_spans(vec![TextSpan {
            letter_spacing: -1.5,
            ..span("\u{202a}שלום abc עולם\u{202c}", 30.0)
        }]);
        request.max_width = Some(260.0);
        request.align = TextAlign::Right;
        let shaped = system.shape(&request);
        assert!((shaped.measurement.width - 149.1123).abs() < 0.01);
    }

    #[test]
    fn negative_tracking_trims_the_rightmost_glyph() {
        let mut system = TextSystem::new();
        let mut request = TextLayoutRequest::from_spans(vec![
            TextSpan {
                letter_spacing: 10.0,
                ..span("H", 40.0)
            },
            TextSpan {
                letter_spacing: -20.0,
                ..span("I", 40.0)
            },
        ]);
        let shaped = system.shape(&request);
        assert!((shaped.measurement.width - shaped.bounds[2]).abs() < 0.01);
        assert!((shaped.bounds[2] - 49.39453).abs() < 0.01);
        assert!(shaped.segments[1].x[1] - shaped.segments[1].x[0] > 5.0);

        request.max_width = Some(200.0);
        request.align = TextAlign::Right;
        let shaped = system.shape(&request);
        assert!((shaped.bounds[0] + shaped.bounds[2] - 200.0).abs() < 0.01);
        assert!((shaped.segments[1].x[1] - 200.0).abs() < 0.01);
    }

    #[test]
    fn newline_only_span_sets_its_empty_line_height() {
        let mut system = TextSystem::new();
        let shaped = system.shape(&TextLayoutRequest::from_spans(vec![
            span("a\n", 16.0),
            span("\n", 80.0),
            span("b", 16.0),
        ]));
        assert_eq!(shaped.counts.line, 3);
        assert!((shaped.measurement.height - 134.4).abs() < 0.01);
        assert!((shaped.segments[1].line_top - (19.2 + 96.0)).abs() < 0.01);
    }

    #[test]
    fn justified_lines_fill_the_width_after_tracking_compensation() {
        let mut system = TextSystem::new();
        let mut request = TextLayoutRequest::from_spans(vec![
            span("a ", 20.0),
            TextSpan {
                letter_spacing: 10.0,
                ..span("one two three four five six seven eight", 20.0)
            },
        ]);
        request.max_width = Some(200.0);
        request.align = TextAlign::Justified;
        let shaped = system.shape(&request);
        // The fourth line is a single word, which has no spaces to widen.
        for line in 0..3 {
            let end = shaped
                .segments
                .iter()
                .filter(|segment| segment.line == line)
                .map(|segment| segment.x[1])
                .fold(f32::NEG_INFINITY, f32::max);
            assert!((end - 200.0).abs() < 0.01, "line {line} ends at {end}");
        }
        assert!(shaped.bounds[0] + shaped.bounds[2] <= 200.01);
    }

    fn ellipsis_glyph(system: &mut TextSystem, font_size: f32) -> u16 {
        let shaped = system.shape(&TextLayoutRequest::from_spans(vec![span(
            "\u{2026}", font_size,
        )]));
        shaped.glyphs[0].glyph.glyph_id
    }

    #[test]
    fn max_lines_end_wrapped_text_in_an_ellipsis() {
        let mut system = TextSystem::new();
        let ellipsis = ellipsis_glyph(&mut system, 20.0);
        let items = |shaped: &ShapedText| -> Vec<(u16, TextItem)> {
            shaped
                .glyphs
                .iter()
                .map(|shaped| (shaped.glyph.glyph_id, shaped.item))
                .collect()
        };
        let mut request =
            TextLayoutRequest::from_spans(vec![span("one two three four five six", 20.0)]);
        request.max_width = Some(100.0);
        let full = system.shape(&request);
        assert!(full.counts.line > 2);
        request.max_lines = Some(2);
        let clipped = system.shape(&request);
        request.overflow = TextOverflow::Ellipsis;
        let ellipsized = system.shape(&request);
        for shaped in [&clipped, &ellipsized] {
            assert_eq!(shaped.counts.line, 2);
            assert!((shaped.measurement.height - 48.0).abs() < 0.01);
            assert!(shaped.measurement.width <= 100.01);
        }
        // The lines shown keep their glyphs' numbers, and the counts end with them.
        let shown = clipped.glyphs.len();
        assert_eq!(items(&clipped), items(&full)[..shown]);
        let last_shown = clipped.glyphs[shown - 1].item;
        assert_eq!(clipped.counts.glyph, last_shown.glyph + 1);
        assert_eq!(clipped.counts.word, last_shown.word + 1);
        // The ellipsis is one more glyph, after the second line's.
        let (last, kept) = ellipsized.glyphs.split_last().unwrap();
        assert_eq!(last.glyph.glyph_id, ellipsis);
        assert_eq!(
            (last.item.line, last.item.glyph),
            (1, ellipsized.counts.glyph - 1)
        );
        assert!(kept.iter().all(|glyph| glyph.glyph.x < last.glyph.x));

        // A word broken to fill the line loses clusters until the ellipsis fits after it.
        let mut request = TextLayoutRequest::from_spans(vec![span("Supercalifragilistic", 20.0)]);
        request.max_width = Some(100.0);
        let first_line = system
            .shape(&request)
            .glyphs
            .iter()
            .filter(|glyph| glyph.item.line == 0)
            .count();
        request.max_lines = Some(1);
        request.overflow = TextOverflow::Ellipsis;
        let shaped = system.shape(&request);
        let (last, kept) = shaped.glyphs.split_last().unwrap();
        assert_eq!(last.glyph.glyph_id, ellipsis);
        assert!(kept.len() < first_line);
        assert!(shaped.measurement.width <= 100.01);
        let kept_end = kept
            .last()
            .map(|glyph| glyph.glyph.x + glyph.glyph.w)
            .unwrap();
        assert!((last.glyph.x - kept_end).abs() < 0.01);
        assert_eq!(shaped.counts.glyph, kept.len() as u32 + 1);
    }

    #[test]
    fn right_to_left_ellipsis_ends_the_line_on_its_left() {
        let mut system = TextSystem::new();
        let ellipsis = ellipsis_glyph(&mut system, 30.0);
        let mut request = TextLayoutRequest::from_spans(vec![span("שלום עולם טוב מאוד", 30.0)]);
        request.max_width = Some(120.0);
        request.max_lines = Some(1);
        request.overflow = TextOverflow::Ellipsis;
        request.align = TextAlign::Right;
        let shaped = system.shape(&request);
        assert_eq!(shaped.counts.line, 1);
        let leftmost = shaped
            .glyphs
            .iter()
            .min_by(|a, b| a.glyph.x.total_cmp(&b.glyph.x))
            .unwrap();
        assert_eq!(leftmost.glyph.glyph_id, ellipsis);
        assert_eq!(leftmost.item.glyph, shaped.counts.glyph - 1);
        assert!(shaped.measurement.width <= 120.01);
        assert!((shaped.bounds[0] + shaped.bounds[2] - 120.0).abs() < 0.01);
        assert!(shaped.segments.iter().all(|segment| segment.rtl));
    }

    #[test]
    fn shrink_fits_a_long_line_by_the_largest_factor() {
        let mut system = TextSystem::new();
        let mut request = TextLayoutRequest::from_spans(vec![
            span("Shrink this long line ", 40.0),
            TextSpan {
                letter_spacing: 2.0,
                ..span("into the box", 30.0)
            },
        ]);
        request.max_width = Some(200.0);
        request.max_lines = Some(1);
        request.min_fit_scale = Some(0.1);
        let shaped = system.shape(&request);
        let scale = shaped.fit_scale;
        assert!(scale > 0.1 && scale < 1.0, "{scale}");
        assert_eq!(shaped.counts.line, 1);
        assert!(shaped.measurement.width <= 200.01);
        assert!((shaped.glyphs[0].glyph.font_size - 40.0 * scale).abs() < 0.001);
        // It measures and draws as the text shaped at that factor, which 1% larger overflows.
        let unfitted = |scale: f32| {
            let mut request = request.scaled(scale);
            request.min_fit_scale = None;
            request
        };
        let again = system.shape(&request);
        assert_eq!(again.measurement, shaped.measurement);
        assert_eq!(system.fits.len(), 1);
        let plain = system.shape(&unfitted(scale));
        assert_eq!(plain.measurement, shaped.measurement);
        assert_eq!(plain.bounds, shaped.bounds);
        assert!((shaped.bounds[2] - shaped.measurement.width).abs() < 0.01);
        let larger = system.shape_lines(&unfitted(scale * 1.01), Wrap::Word, false);
        assert!(request.fit_ratio(&larger) < 1.0);
        // Text that already fits never grows.
        request.max_width = Some(2000.0);
        assert_eq!(system.shape(&request).fit_scale, 1.0);
    }

    #[test]
    fn text_that_does_not_fit_at_the_smallest_factor_overflows() {
        let mut system = TextSystem::new();
        let ellipsis = ellipsis_glyph(&mut system, 36.0);
        let mut request =
            TextLayoutRequest::from_spans(vec![span("Too long to fit in one line", 40.0)]);
        request.max_width = Some(200.0);
        request.max_lines = Some(1);
        request.overflow = TextOverflow::Ellipsis;
        request.min_fit_scale = Some(0.9);
        let shaped = system.shape(&request);
        assert_eq!(shaped.fit_scale, 0.9);
        assert_eq!(shaped.counts.line, 1);
        let last = shaped.glyphs.last().unwrap();
        assert_eq!(last.glyph.glyph_id, ellipsis);
        assert!((last.glyph.font_size - 36.0).abs() < 0.001);
    }

    #[test]
    fn right_to_left_ellipsis_measures_without_the_rightmost_tracking() {
        let mut system = TextSystem::new();
        let mut request = TextLayoutRequest::from_spans(vec![
            TextSpan {
                letter_spacing: 10.0,
                ..span("שלום ", 30.0)
            },
            span("עולם\nעוד", 30.0),
        ]);
        request.max_width = Some(500.0);
        request.max_lines = Some(1);
        request.overflow = TextOverflow::Ellipsis;
        request.align = TextAlign::Right;
        let shaped = system.shape(&request);
        let [x, _, width, _] = shaped.bounds;
        assert!((shaped.measurement.width - width).abs() < 0.01);
        assert!((x + width - 500.0).abs() < 0.01, "{x} + {width}");
        let right = shaped
            .segments
            .iter()
            .map(|segment| segment.x[1])
            .fold(f32::NEG_INFINITY, f32::max);
        assert!((right - 500.0).abs() < 0.01, "{right}");
    }

    #[test]
    fn an_ellipsis_alone_on_its_line_takes_the_decoration_of_the_text_it_replaced() {
        let mut system = TextSystem::new();
        let mut request = TextLayoutRequest::from_spans(vec![span("Hello world", 20.0)]);
        request.max_width = Some(12.0);
        request.max_lines = Some(1);
        request.overflow = TextOverflow::Ellipsis;
        let shaped = system.shape(&request);
        assert_eq!(shaped.glyphs.len(), 1);
        let [segment] = &shaped.segments[..] else {
            panic!("expected the ellipsis alone");
        };
        let mut instances = Vec::new();
        let highlight = TextDecoration {
            shifted: false,
            kind: TextDecorationKind::Highlight {
                padding: [0.0; 2],
                radius: 0.0,
            },
            spans: 0..1,
            extent: [0.0, 1.0],
        };
        shaped.decorate(&highlight, [0.0; 2], |_| 0, &mut instances);
        let [rect] = &instances[..] else {
            panic!("expected one rect");
        };
        assert!((rect.rect[0] - segment.x[0]).abs() < 0.01);
        assert!((rect.rect[2] - (segment.x[1] - segment.x[0])).abs() < 0.01);
    }

    #[test]
    fn extents_follow_logical_order_in_right_to_left_text() {
        let mut system = TextSystem::new();
        let rects = |shaped: &ShapedText, extent: [f32; 2], spans: Range<usize>| {
            let decoration = TextDecoration {
                shifted: false,
                kind: TextDecorationKind::Underline(TextLineDecoration {
                    style: TextLineStyle::Solid,
                    thickness: None,
                    offset: None,
                }),
                spans,
                extent,
            };
            let mut instances = Vec::new();
            shaped.decorate(&decoration, [0.0; 2], |_| 0, &mut instances);
            instances
                .iter()
                .map(|instance| [instance.rect[0], instance.rect[0] + instance.rect[2]])
                .collect::<Vec<_>>()
        };
        let close = |a: f32, b: f32| (a - b).abs() < 0.01;

        // The first span of right-to-left text is on the right, and is reached first.
        let shaped = system.shape(&TextLayoutRequest::from_spans(vec![
            span("של", 30.0),
            span("ום", 30.0),
        ]));
        let first = shaped.segments.iter().find(|s| s.span == 0).unwrap();
        let second = shaped.segments.iter().find(|s| s.span == 1).unwrap();
        assert!(first.rtl && first.x[0] > second.x[0]);
        let [rect] = rects(&shaped, [0.0, 0.2], 0..2)[..] else {
            panic!("expected one rect");
        };
        assert!(close(rect[1], first.x[1]) && rect[0] >= first.x[0] - 0.01);
        let [rect] = rects(&shaped, [0.0, 0.5], 0..2)[..] else {
            panic!("expected one rect");
        };
        assert!(close(rect[0], first.x[0]) && close(rect[1], first.x[1]));

        // An ellipsis ends right-to-left text on its left, and is reached last.
        let mut request = TextLayoutRequest::from_spans(vec![span("שלום\nשלום", 30.0)]);
        request.max_lines = Some(1);
        request.overflow = TextOverflow::Ellipsis;
        let shaped = system.shape(&request);
        let ellipsis = shaped.segments.iter().find(|s| s.bytes.is_empty()).unwrap();
        let text = shaped
            .segments
            .iter()
            .find(|s| !s.bytes.is_empty())
            .unwrap();
        assert!(ellipsis.x[0] < text.x[0]);
        let total = (ellipsis.x[1] - ellipsis.x[0]) + (text.x[1] - text.x[0]);
        let reach = (text.x[1] - text.x[0]) / total;
        let [rect] = rects(&shaped, [0.0, reach * 0.5], 0..1)[..] else {
            panic!("expected one rect");
        };
        assert!(close(rect[1], text.x[1]) && rect[0] > text.x[0]);
        let [rect] = rects(&shaped, [0.0, 1.0], 0..1)[..] else {
            panic!("expected one rect");
        };
        assert!(close(rect[0], ellipsis.x[0]) && close(rect[1], text.x[1]));
    }

    #[test]
    fn an_ellipsis_at_a_span_boundary_is_decorated_by_extents_reaching_it() {
        let mut system = TextSystem::new();
        let mut request =
            TextLayoutRequest::from_spans(vec![span("Hello", 20.0), span("\nmore", 20.0)]);
        request.max_lines = Some(1);
        request.overflow = TextOverflow::Ellipsis;
        let shaped = system.shape(&request);
        let ellipsis = shaped.segments.last().unwrap();
        assert_eq!((ellipsis.span, ellipsis.bytes.clone()), (0, 5..5));
        let underline = |spans: Range<usize>, extent: [f32; 2]| {
            let decoration = TextDecoration {
                shifted: false,
                kind: TextDecorationKind::Underline(TextLineDecoration {
                    style: TextLineStyle::Solid,
                    thickness: None,
                    offset: None,
                }),
                spans,
                extent,
            };
            let mut instances = Vec::new();
            shaped.decorate(&decoration, [0.0; 2], |_| 0, &mut instances);
            instances
                .iter()
                .map(|instance| instance.rect[0] + instance.rect[2])
                .collect::<Vec<_>>()
        };
        // The ellipsis is part of the run's width, so reaching the end of the text it
        // follows leaves it undecorated and reaching the end of the run covers it.
        let text = &shaped.segments[0];
        let total = ellipsis.x[1] - text.x[0];
        let before_ellipsis = (text.x[1] - text.x[0]) / total;
        let [end] = underline(0..2, [0.0, before_ellipsis])[..] else {
            panic!("expected one rect");
        };
        assert!((end - text.x[1]).abs() < 0.01, "{end}");
        for spans in [0..1, 0..2] {
            let [end] = underline(spans, [0.0, 1.0])[..] else {
                panic!("expected one rect");
            };
            assert!((end - ellipsis.x[1]).abs() < 0.01, "{end}");
        }
        // Starting at the end of the text covers the ellipsis alone.
        let [end] = underline(0..2, [before_ellipsis, 1.0])[..] else {
            panic!("expected one rect");
        };
        assert!((end - ellipsis.x[1]).abs() < 0.01, "{end}");
        // The second span is hidden, and has no width to decorate.
        assert!(underline(1..2, [0.0, 1.0]).is_empty());
    }

    #[test]
    fn max_height_drops_lines_and_a_trailing_empty_line_counts() {
        let mut system = TextSystem::new();
        let ellipsis = ellipsis_glyph(&mut system, 20.0);
        let mut request = TextLayoutRequest::from_spans(vec![span("one\r\ntwo\r\n", 20.0)]);
        request.line_height = Some(20.0);
        assert_eq!(system.shape(&request).counts.line, 3);
        request.max_height = Some(45.0);
        let clipped = system.shape(&request);
        assert_eq!(clipped.counts.line, 2);
        assert_eq!(clipped.measurement.height, 40.0);
        assert_eq!(clipped.counts.glyph, 6);
        // The empty line dropped still ends the line before it in an ellipsis.
        request.overflow = TextOverflow::Ellipsis;
        let shaped = system.shape(&request);
        let last = shaped.glyphs.last().unwrap();
        assert_eq!(last.glyph.glyph_id, ellipsis);
        assert_eq!((last.item.line, last.item.glyph), (1, 6));
        assert_eq!(shaped.counts.glyph, 7);
    }

    #[test]
    fn shrink_reaches_a_floor_below_one_percent() {
        let mut system = TextSystem::new();
        let mut request = TextLayoutRequest::from_spans(vec![span("HH", 2000.0)]);
        request.max_width = Some(15.0);
        request.max_lines = Some(1);
        request.min_fit_scale = Some(8.0 / 2000.0);
        let shaped = system.shape(&request);
        assert_eq!(shaped.glyphs.len(), 2);
        assert!(shaped.fit_scale < 0.01, "{}", shaped.fit_scale);
        assert!(shaped.measurement.width <= 15.01);
    }

    #[test]
    fn a_cached_fit_shapes_as_the_fit_did() {
        let mut system = TextSystem::new();
        let mut request = TextLayoutRequest::from_spans(vec![span("HH", 40.0)]);
        request.max_width = Some(17.0);
        request.max_lines = Some(1);
        request.min_fit_scale = Some(0.2);
        let fitted = system.shape(&request);
        let cached = system.shape(&request);
        assert_eq!(fitted.counts.glyph, 2);
        assert_eq!(cached.counts.glyph, 2);
        assert_eq!(cached.measurement, fitted.measurement);
    }

    #[test]
    fn shrink_keeps_the_size_when_only_paragraphs_overflow() {
        let mut system = TextSystem::new();
        let mut request = TextLayoutRequest::from_spans(vec![span("A\nB", 40.0)]);
        request.max_lines = Some(1);
        request.min_fit_scale = Some(0.2);
        let shaped = system.shape(&request);
        assert_eq!(shaped.fit_scale, 1.0);
        assert_eq!(shaped.counts.line, 1);
    }

    fn featured(text: &str, features: &[(&[u8; 4], u32)]) -> TextSpan {
        TextSpan {
            font_features: features
                .iter()
                .map(|(tag, value)| FontFeature {
                    tag: **tag,
                    value: *value,
                })
                .collect(),
            ..span(text, 40.0)
        }
    }

    #[test]
    fn features_change_advances_per_span() {
        let mut system = TextSystem::new();
        let mut width =
            |spans: Vec<TextSpan>| system.measure(&TextLayoutRequest::from_spans(spans)).width;
        // Roboto's digits are tabular unless `pnum` asks for proportional ones.
        let tabular = width(vec![featured("1111 1111", &[])]);
        let proportional = width(vec![featured("1111 1111", &[(b"pnum", 1)])]);
        assert!(proportional < tabular - 1.0);
        let mixed = width(vec![
            featured("1111 ", &[(b"pnum", 1)]),
            featured("1111", &[]),
        ]);
        assert!(proportional < mixed && mixed < tabular);
    }

    #[test]
    fn disabling_ligatures_keeps_both_glyphs() {
        let mut system = TextSystem::new();
        let mut glyphs = |features| {
            system
                .layout(&TextLayoutRequest::from_spans(vec![featured(
                    "fi", features,
                )]))
                .glyphs
                .len()
        };
        assert_eq!(glyphs(&[]), 1);
        assert_eq!(glyphs(&[(b"liga", 0)]), 2);
    }

    #[test]
    fn uppercase_keeps_numbering_in_the_transformed_text() {
        let mut system = TextSystem::new();
        let shaped = system.shape(&TextLayoutRequest::from_spans(vec![
            TextSpan {
                transform: TextTransform::Uppercase,
                ..span("Maßﬁ ", 30.0)
            },
            span("end", 30.0),
        ]));
        // "ß" becomes "SS" and "ﬁ" "FI", so the span is "MASSFI " in seven bytes, not eight.
        let layout = shaped.place([0.0; 2], [1.0; 4]);
        assert_eq!(shaped.span_starts, [0, 7, 10]);
        assert_eq!(shaped.counts.glyph, 10);
        assert_eq!(shaped.counts.word, 2);
        let items: Vec<_> = layout
            .glyphs
            .iter()
            .map(|glyph| (glyph.item.glyph, glyph.item.word, glyph.item.span))
            .collect();
        let expected: Vec<_> = (0..10)
            .map(|glyph| (glyph, u32::from(glyph >= 7), u32::from(glyph >= 7)))
            .collect();
        assert_eq!(items, expected);
        assert_eq!(shaped.segments[0].bytes, 0..7);
        assert_eq!(shaped.segments[1].bytes, 0..3);

        // An underline over the second span starts where its first glyph does, and one over
        // half of the first covers half of its width.
        let underline = |spans, extent| TextDecoration {
            kind: TextDecorationKind::Underline(TextLineDecoration {
                style: TextLineStyle::Solid,
                thickness: None,
                offset: None,
            }),
            spans,
            shifted: false,
            extent,
        };
        let rect = |decoration| {
            let mut instances = Vec::new();
            shaped.decorate(&decoration, [0.0; 2], |_| 0, &mut instances);
            instances[0].rect
        };
        let [second, first] = [
            rect(underline(1..2, [0.0, 1.0])),
            rect(underline(0..1, [0.0, 0.5])),
        ];
        assert!((second[0] - shaped.segments[1].x[0]).abs() < 0.01);
        let width = shaped.segments[0].x[1] - shaped.segments[0].x[0];
        assert!((first[2] - width * 0.5).abs() < 0.01);
    }

    #[test]
    fn capitalize_uppercases_each_word_across_spans() {
        let capitalized = |texts: &[&str], transform| {
            let spans: Vec<_> = texts
                .iter()
                .map(|text| TextSpan {
                    transform,
                    ..TextSpan::new(*text)
                })
                .collect();
            transformed_texts(&spans).concat()
        };
        assert_eq!(
            capitalized(
                &["hello wor", "ld, don't (stop)"],
                TextTransform::Capitalize
            ),
            "Hello World, Don't (Stop)"
        );
        assert_eq!(
            capitalized(&["Hello ", "WORLD"], TextTransform::Lowercase),
            "hello world"
        );
    }

    #[test]
    fn lowercase_decides_final_sigma_across_spans() {
        let lowercased = |texts: &[&str]| {
            let spans: Vec<_> = texts
                .iter()
                .map(|text| TextSpan {
                    transform: TextTransform::Lowercase,
                    ..TextSpan::new(*text)
                })
                .collect();
            transformed_texts(&spans).concat()
        };
        for whole in ["ΟΣ", "ΣΑ", "Ο.Σ.Α", "ΟΣ\u{301}", "ΣΣ", "İΣ", "A\u{345}Σ"] {
            let expected = whole.to_lowercase();
            for (split, _) in whole.char_indices().skip(1) {
                let (first, second) = whole.split_at(split);
                assert_eq!(lowercased(&[first, second]), expected);
            }
        }
        assert_eq!(lowercased(&["Ο", "Σ"]), "ος");
        assert_eq!(lowercased(&["Σ", "Α"]), "σα");
    }

    #[test]
    fn baseline_shift_moves_a_span_but_not_the_line() {
        let mut system = TextSystem::new();
        let request = |shift| {
            TextLayoutRequest::from_spans(vec![
                span("ab", 30.0),
                TextSpan {
                    baseline_shift: shift,
                    ..span("cd", 30.0)
                },
            ])
        };
        let plain = system.shape(&request(0.0));
        let raised = system.shape(&request(12.0));
        assert_eq!(raised.measurement, plain.measurement);
        let (plain_layout, raised_layout) = (
            plain.place([0.0; 2], [1.0; 4]),
            raised.place([0.0; 2], [1.0; 4]),
        );
        for (index, (before, after)) in plain_layout
            .glyphs
            .iter()
            .zip(&raised_layout.glyphs)
            .enumerate()
        {
            let lifted = if index < 2 { 0.0 } else { 12.0 };
            assert!((before.position[1] - after.position[1] - lifted).abs() < 0.01);
            assert_eq!(before.position[0], after.position[0]);
        }
        assert_eq!(
            raised.segments[1].baseline,
            plain.segments[1].baseline - 12.0
        );
        assert_eq!(raised.segments[0].baseline, plain.segments[0].baseline);

        // The bounds and the span's box rise with it, and its own underline does too, while
        // the node's stays on the line.
        assert_eq!(raised.bounds[1], -12.0);
        assert!((raised.bounds[3] - (plain.bounds[3] + 12.0)).abs() < 0.01);
        let [x, y, ..] = raised.span_boxes[1].unwrap();
        let [plain_x, plain_y, ..] = plain.span_boxes[1].unwrap();
        assert_eq!((x, y), (plain_x, plain_y - 12.0));
        let underline_top = |shifted| {
            let decoration = TextDecoration {
                kind: TextDecorationKind::Underline(TextLineDecoration {
                    style: TextLineStyle::Solid,
                    thickness: None,
                    offset: None,
                }),
                spans: 1..2,
                shifted,
                extent: [0.0, 1.0],
            };
            let mut instances = Vec::new();
            raised.decorate(&decoration, [0.0; 2], |_| 0, &mut instances);
            instances[0].rect[1]
        };
        assert!((underline_top(false) - underline_top(true) - 12.0).abs() < 0.01);
    }

    #[test]
    fn limits_and_fit_count_paragraph_spacing_and_baseline_shifts() {
        let mut system = TextSystem::new();
        // Two 24 px lines fit 50 px, but not with 20 px between their paragraphs.
        let mut request = TextLayoutRequest::from_spans(vec![
            span("a\n", 20.0),
            TextSpan {
                baseline_shift: 10.0,
                ..span("b", 20.0)
            },
        ]);
        request.paragraph_spacing = 20.0;
        request.max_height = Some(50.0);
        assert_eq!(system.shape(&request).counts.line, 1);

        // Shrinking scales the spacing and the shift with the text.
        request.min_fit_scale = Some(0.1);
        let fitted = system.shape(&request);
        let scale = fitted.fit_scale;
        assert_eq!(fitted.counts.line, 2);
        assert!(fitted.measurement.height <= 50.01);
        assert!(
            (fitted.measurement.height - (48.0 + 20.0) * scale).abs() < 0.05,
            "{} at {scale}",
            fitted.measurement.height
        );
        let shift = fitted
            .segments
            .iter()
            .find(|segment| segment.span == 1)
            .unwrap()
            .shift;
        assert!((shift - 10.0 * scale).abs() < 0.01, "{shift} at {scale}");
    }

    #[test]
    fn paragraph_spacing_separates_paragraphs_only() {
        let mut system = TextSystem::new();
        let mut shape = |text: &str, spacing: f32, max_width: Option<f32>| {
            let mut request = TextLayoutRequest::from_spans(vec![span(text, 20.0)]);
            request.paragraph_spacing = spacing;
            request.max_width = max_width;
            system.shape(&request)
        };
        let base = shape("a", 0.0, None).measurement.height;
        assert_eq!(shape("a", 9.0, None).measurement.height, base);
        assert_eq!(
            shape("a\nb\nc", 9.0, None).measurement.height,
            shape("a\nb\nc", 0.0, None).measurement.height + 18.0
        );
        // A wrapped paragraph's lines are not separated.
        let wrapped = shape("aaa bbb", 9.0, Some(40.0));
        assert_eq!(wrapped.counts.line, 2);
        assert_eq!(wrapped.measurement.height, base * 2.0);
        // Later paragraphs, their glyphs and boxes move down together.
        let spaced = shape("a\nb", 9.0, None);
        let plain = shape("a\nb", 0.0, None);
        assert_eq!(
            spaced.segments[1].line_top,
            plain.segments[1].line_top + 9.0
        );
        let (spaced, plain) = (
            spaced.place([0.0; 2], [1.0; 4]),
            plain.place([0.0; 2], [1.0; 4]),
        );
        assert_eq!(
            spaced.glyphs[1].position[1],
            plain.glyphs[1].position[1] + 9.0
        );
        assert_eq!(spaced.glyphs[1].pivot[1], plain.glyphs[1].pivot[1] + 9.0);
    }
}
