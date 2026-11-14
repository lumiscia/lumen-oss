use super::*;
use crate::{TextDecoration, TextDecorationKind, TextLineDecoration, TextLineStyle};

#[test]
fn changed_text_reuses_kept_shapes_without_pushing_them_out() {
    let mut system = TextSystem::new();
    let measured: Vec<_> = (0..SHAPE_CACHE)
        .map(|index| TextLayoutRequest::new(format!("Measured {index}")))
        .collect();
    let kept: Vec<_> = measured.iter().map(|text| system.shape(text)).collect();

    assert!(Arc::ptr_eq(&system.shape_changed(&measured[0]), &kept[0]));
    system.shape_changed(&TextLayoutRequest::new("Animated"));

    for (text, kept) in measured.iter().zip(&kept) {
        assert!(Arc::ptr_eq(&system.shape(text), kept));
    }
}

#[test]
fn shape_cache_evicts_the_oldest_request() {
    let mut system = TextSystem::new();
    let first = TextLayoutRequest::new("First");
    let shaped = system.shape(&first);
    system.measure(&first);
    assert!(Arc::ptr_eq(&system.shape(&first), &shaped));
    for index in 0..SHAPE_CACHE {
        system.measure(&TextLayoutRequest::new(format!("Text {index}")));
    }
    assert_eq!(system.shapes.len(), SHAPE_CACHE);
    assert!(!Arc::ptr_eq(&system.shape(&first), &shaped));
    assert_eq!(system.shapes.len(), SHAPE_CACHE);
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
fn letter_spacing_widens_advance_and_line_height_spaces_lines() {
    let mut system = TextSystem::new();
    let mut request = TextLayoutRequest::new("Spacing");
    request.spans[0].font_size = 40.0;
    let base = system.shape(&request);
    let base_glyphs = base.place([0.0; 2]);
    let glyph_x = |layout: &TextLayout, index: usize| layout.glyphs[index].position[0];

    request.spans[0].letter_spacing = 6.0;
    let tracked = system.shape(&request);
    let count = base.glyph_count();
    assert!(
        (tracked.measurement.width - base.measurement.width - 6.0 * (count - 1) as f32).abs()
            < 0.01
    );
    assert!(
        (glyph_x(&tracked.place([0.0; 2]), count - 1)
            - glyph_x(&base_glyphs, count - 1)
            - 6.0 * (count - 1) as f32)
            .abs()
            < 0.01
    );

    request.spans[0].letter_spacing = -2.0;
    let tight = system.shape(&request);
    assert!(tight.measurement.width < base.measurement.width);

    let line_gap = |system: &mut TextSystem, line_height| {
        let mut request = TextLayoutRequest::new("a\nb");
        request.spans[0].font_size = 40.0;
        request.line_height = line_height;
        let layout = system.shape(&request).place([0.0; 2]);
        layout.glyphs[1].position[1] - layout.glyphs[0].position[1]
    };
    assert!((line_gap(&mut system, None) - 48.0).abs() < 0.01);
    assert!((line_gap(&mut system, Some(80.0)) - 80.0).abs() < 0.01);
}

#[test]
fn trailing_letter_spacing_is_excluded_from_measurement_and_alignment() {
    let mut system = TextSystem::new();
    let h_x = |system: &mut TextSystem, align, spacing| {
        let mut request = TextLayoutRequest::new("H");
        request.spans[0].font_size = 40.0;
        request.max_width = Some(200.0);
        request.align = align;
        request.spans[0].letter_spacing = spacing;
        system.shape(&request).place([0.0; 2]).glyphs[0].position[0]
    };
    for align in [TextAlign::Left, TextAlign::Center, TextAlign::Right] {
        let base = h_x(&mut system, align, 0.0);
        for spacing in [20.0, -10.0] {
            assert!((h_x(&mut system, align, spacing) - base).abs() < 0.01);
        }
    }

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
fn letter_spacing_wraps_on_trimmed_line_width() {
    let mut system = TextSystem::new();
    let mut request = TextLayoutRequest::new("HH");
    request.spans[0].font_size = 40.0;
    request.spans[0].letter_spacing = 20.0;
    let natural = system.measure(&request).width;
    let line_count = |system: &mut TextSystem, request: &mut TextLayoutRequest, width| {
        request.max_width = Some(width);
        let layout = system.shape(request).place([0.0; 2]);
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

    // A span with no text does not move where the others wrap.
    request.spans.push(TextSpan {
        letter_spacing: -40.0,
        ..span("", 40.0)
    });
    assert_eq!(line_count(&mut system, &mut request, natural + 0.5), 1);
    assert_eq!(line_count(&mut system, &mut request, natural - 0.5), 2);
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

fn items(system: &mut TextSystem, request: &TextLayoutRequest) -> (Vec<[u32; 3]>, TextItem) {
    let layout = system.shape(request).place([0.0; 2]);
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
    let words: Vec<_> = wrapped
        .iter()
        .map(|[_, word, line]| (*word, *line))
        .collect();
    assert_eq!(&words[..3], [(0, 0); 3]);
    assert_eq!(*words.last().unwrap(), (2, 2));
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
    let layout = system.shape(&request).place([0.0; 2]);
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

pub(crate) fn span(text: &str, font_size: f32) -> TextSpan {
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
        let layout = shaped.place([0.0, 0.0]);
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
    let layout = system.shape(&request).place([0.0; 2]);

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
    assert!((system.measure(&request).height - (57.6 + 19.2)).abs() < 0.01);
    let lines: Vec<_> = layout.glyphs.iter().map(|glyph| glyph.item.line).collect();
    assert_eq!(lines[9..], [1; 5]);
}

#[test]
fn segments_follow_spans_across_wrapped_lines() {
    let mut system = TextSystem::new();
    let text = "beta gamma delta";
    let mut request = TextLayoutRequest::from_spans(vec![span("alpha ", 20.0), span(text, 28.0)]);
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
    let base = system.shape(&tracked(0.0, 0.0));
    let shaped = system.shape(&tracked(10.0, 30.0));
    // The last glyph's own spacing is trimmed.
    let expected = base.measurement.width + 10.0 * 2.0 + 30.0;
    assert!((shaped.measurement.width - expected).abs() < 0.01);
    assert!((shaped.bounds[2] - expected).abs() < 0.01);
    let x = |shaped: &ShapedText| shaped.place([0.0; 2]).glyphs[3].position[0];
    assert!((x(&shaped) - x(&base) - 50.0).abs() < 0.01);

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
fn plain_text_keeps_cr_and_lf_line_endings() {
    let mut system = TextSystem::new();
    let trailing = system.shape(&TextLayoutRequest::new("a\n"));
    assert_eq!(trailing.counts.line, 2);
    assert!((trailing.measurement.height - 38.4).abs() < 0.01);
    let carriage = system.shape(&TextLayoutRequest::new("a\rB"));
    assert_eq!(carriage.counts.line, 2);
    assert_eq!(carriage.place([0.0; 2]).glyphs.len(), 2);
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
        .place([0.0; 2])
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
    request.align = TextAlign::Justify;
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
    request.align = TextAlign::Justify;
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
    request.align = TextAlign::Justify;
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
            .shape(&TextLayoutRequest::from_spans(vec![featured(
                "fi", features,
            )]))
            .place([0.0; 2])
            .glyphs
            .len()
    };
    assert_eq!(glyphs(&[]), 1);
    assert_eq!(glyphs(&[(b"liga", 0)]), 2);
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
    let (plain_layout, raised_layout) = (plain.place([0.0; 2]), raised.place([0.0; 2]));
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
    let (spaced, plain) = (spaced.place([0.0; 2]), plain.place([0.0; 2]));
    assert_eq!(
        spaced.glyphs[1].position[1],
        plain.glyphs[1].position[1] + 9.0
    );
    assert_eq!(spaced.glyphs[1].pivot[1], plain.glyphs[1].pivot[1] + 9.0);
}
