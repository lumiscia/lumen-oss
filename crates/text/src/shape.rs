use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

use cosmic_text::{
    Align, Attrs, AttrsList, Buffer, CacheKey, CacheKeyFlags, Family, FeatureTag, FontFeatures,
    LayoutGlyph, LineEnding, LineIter, Metrics, Shaping, Style, Weight, Wrap,
};

use crate::decorate::TextSpanMetrics;
use crate::transform::{TextTransform, transformed_texts};
use crate::{DEFAULT_FONT_FAMILY, TextSystem};

/// Most shapes a fit takes, counting the final one of text that fits at no factor tried.
const FIT_SHAPES: usize = 12;
/// Smallest factor a fit shrinks text by when its requested floor is not a positive number.
const MIN_FIT_SCALE: f32 = 0.01;
/// Most recently shaped requests a text system keeps.
const SHAPE_CACHE: usize = 32;
/// Pixels by which lines may pass a limit and still fit it.
const FIT_TOLERANCE: f32 = 0.01;
/// Stands in for the byte offset of an ellipsis' cluster, which is past every other.
const ELLIPSIS_CLUSTER: usize = usize::MAX;

impl TextSystem {
    pub fn measure(&mut self, request: &TextLayoutRequest) -> TextMeasurement {
        self.shape(request).measurement
    }

    /// Shapes text without placing it, so it can be placed at many origins. Measurement, bounds
    /// and drawing of the same text share one recent result.
    pub fn shape(&mut self, request: &TextLayoutRequest) -> Arc<ShapedText> {
        if let Some(shaped) = self.kept_shape(request) {
            return shaped;
        }
        let shaped = Arc::new(self.shape_fitted(request));
        if self.shapes.len() == SHAPE_CACHE {
            self.shapes.remove(0);
        }
        self.shapes.push((request.clone(), Arc::clone(&shaped)));
        shaped
    }

    /// Shapes text whose inputs just changed, reusing a kept shape without keeping a new one, so
    /// text that changes every frame can't push out text that is measured every frame.
    pub fn shape_changed(&mut self, request: &TextLayoutRequest) -> Arc<ShapedText> {
        self.kept_shape(request)
            .unwrap_or_else(|| Arc::new(self.shape_fitted(request)))
    }

    fn kept_shape(&self, request: &TextLayoutRequest) -> Option<Arc<ShapedText>> {
        self.shapes
            .iter()
            .find(|(cached, _)| cached == request)
            .map(|(_, shaped)| Arc::clone(shaped))
    }

    fn shape_fitted(&mut self, request: &TextLayoutRequest) -> ShapedText {
        let Some(min_scale) = request.min_fit_scale.filter(|_| request.is_limited()) else {
            return self.shape_lines(request, Wrap::WordOrGlyph, true);
        };
        let min_scale = if min_scale > 0.0 {
            min_scale.min(1.0)
        } else {
            MIN_FIT_SCALE
        };
        let (scale, fitted) = self.fit(request, min_scale);
        // Text that fits keeps its words whole, as the fit shaped them, even where it overflows
        // by less than the fit's tolerance.
        let wrap = if fitted.is_some() {
            Wrap::Word
        } else {
            Wrap::WordOrGlyph
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
                // Fallback goes by slot, which would make this the latest font loaded.
                let family = if self.has_family(&span.font_family) {
                    span.font_family.as_str()
                } else {
                    DEFAULT_FONT_FAMILY
                };
                let mut attrs = Attrs::new()
                    .family(Family::Name(family))
                    .weight(Weight(span.font_weight))
                    .style(match span.font_style {
                        TextFontStyle::Italic => Style::Italic,
                        TextFontStyle::Oblique => Style::Oblique,
                        TextFontStyle::Normal => Style::Normal,
                    })
                    .cache_key_flags(CacheKeyFlags::DISABLE_HINTING)
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
        // cosmic-text adds tracking after every glyph, including the last on a
        // line, so every line is its last glyph's `letter_spacing` wider than its
        // ink. Widening the layout width by the narrowest spacing of the spans with
        // text makes wrapping see trimmed line widths; lines ending in a wider spacing
        // wrap early by the difference. cosmic-text clamps negative widths to zero, so
        // a negative spacing larger than `max_width` wraps less than it should.
        let widening = spans
            .iter()
            .filter(|span| !span.text.is_empty())
            .map(|span| span.letter_spacing)
            .reduce(f32::min)
            .unwrap_or(0.0);
        let layout_width = request.max_width.map(|width| width + widening);
        buffer.set_size(layout_width, None);
        buffer.set_wrap(if request.max_width.is_some() {
            wrap
        } else {
            Wrap::None
        });
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
                TextAlign::Justify => Align::Justified,
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

        let spacing = |glyph: &LayoutGlyph| spans[glyph.metadata].letter_spacing;
        let lift = |span: usize| spans[span].baseline_shift;
        let align_factor = match request.align {
            TextAlign::Center => 0.5,
            TextAlign::Right => 1.0,
            TextAlign::Left | TextAlign::Justify => 0.0,
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
        let max_width = request.max_width;
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
                && request.align == TextAlign::Justify
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
                let span = glyph.metadata;
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
            let span = &mut span_boxes[segment.span as usize];
            *span = Some(span.map_or(edges, |a| {
                [
                    a[0].min(edges[0]),
                    a[1].min(edges[1]),
                    a[2].max(edges[2]),
                    a[3].max(edges[3]),
                ]
            }));
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
        let spacing = |glyph: &LayoutGlyph| spans[glyph.metadata].letter_spacing;
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
            let span = last.map_or(fallback_span, |glyph| glyph.metadata);
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
}

/// A run of text with one font and spacing.
#[derive(Debug, Clone, PartialEq)]
pub struct TextSpan {
    pub text: String,
    pub font_family: String,
    pub font_size: f32,
    /// From 1 to 1000.
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
    /// Width lines wrap at, which must be positive.
    pub max_width: Option<f32>,
    /// Line pitch in pixels; `None` is 1.2 × the largest font size on each line.
    pub line_height: Option<f32>,
    /// Extra space in pixels after each paragraph but the last.
    pub paragraph_spacing: f32,
    pub align: TextAlign,
    /// Most lines to show.
    pub max_lines: Option<u32>,
    /// Most height of the lines to show, which must not be negative.
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
            max_lines: None,
            max_height: None,
            overflow: TextOverflow::Clip,
            min_fit_scale: None,
        }
    }

    fn is_limited(&self) -> bool {
        self.max_width.is_some() || self.max_lines.is_some() || self.max_height.is_some()
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
        let wraps = self.max_width.is_some();
        let TextMeasurement { width, height } = shaped.measurement;
        let mut ratio: f32 = 1.0;
        if let Some(max) = self.max_width
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
            let heights = max / height;
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
    Justify,
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
    pub(crate) span_starts: Vec<usize>,
    /// Lines the text has however wide its lines may be.
    paragraphs: u32,
    pub(crate) glyphs: Vec<ShapedGlyph>,
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
pub(crate) struct ShapedGlyph {
    pub(crate) glyph: LayoutGlyph,
    pub(crate) line_y: f32,
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
    pub fn place(&self, origin: [f32; 2]) -> TextLayout {
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
                    item: shaped.item,
                    pivot: [shaped.pivot[0] + origin[0], shaped.pivot[1] + origin[1]],
                }
            })
            .collect();
        TextLayout {
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
}

#[derive(Debug, Clone, PartialEq)]
pub struct TextLayout {
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
    pub item: TextItem,
    /// Centre of the glyph's cluster: the middle of its advance and of its line.
    pub pivot: [f32; 2],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub struct GlyphKey(pub CacheKey);

#[cfg(test)]
#[path = "shape_tests.rs"]
pub(crate) mod tests;
