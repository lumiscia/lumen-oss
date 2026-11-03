use std::ops::Range;

use crate::gpu::{
    GpuGlyphInstance, RECT_BOX, RECT_DASHED, RECT_DOTTED, RECT_DOUBLE, RECT_JOINED_LEFT,
    RECT_JOINED_RIGHT, RECT_SOLID, RECT_WAVY,
};
use crate::shape::{ShapedText, TextSegment};

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

impl ShapedText {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shape::tests::span;
    use crate::{RECT_SHAPE_SHIFT, TextLayoutRequest, TextOverflow, TextSystem};

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
}
