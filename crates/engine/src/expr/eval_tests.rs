use super::*;
use crate::{
    error::ExpressionError,
    graph::Graph,
    node::{DelegateEvaluable, NodeId, NodeKind, ParamValue, source::text::Text},
};

fn test_context() -> ExpressionContext<'static> {
    ExpressionContext {
        frame: 48.0,
        fps: 24.0,
        width: 1920,
        height: 1080,
        graph: None,
        media: None,
        glyph: None,
        instance: None,
        props: None,
        held: None,
    }
}

#[test]
fn evaluates_globals_from_expression_context() {
    for (source, expected) in [
        ("time", 2.0),
        ("fps", 24.0),
        ("width", 1920.0),
        ("height", 1080.0),
    ] {
        assert_eq!(
            evaluate(source, 48).unwrap(),
            ExpressionValue::Number(expected),
            "{source}"
        );
    }
}

#[test]
fn glyph_globals_read_the_glyph_scope_and_fail_outside_it() {
    let source = "glyph + word * 10 + line * 100 + glyph_count * 1000 + word_count * 10000 \
                      + line_count * 100000 + span * 1000000 + span_count * 10000000";
    let expression = Expression::parse(source).unwrap();
    let mut ctx = test_context();
    let error = expression.evaluate(&ctx).unwrap_err().to_string();
    assert!(error.contains("`glyph` is only available"), "{error}");
    let item = |glyph, word, line, span| lumen_text::TextItem {
        glyph,
        word,
        line,
        span,
    };
    ctx.glyph = Some(crate::expr::GlyphScope {
        item: item(1, 2, 3, 7),
        counts: item(4, 5, 6, 8),
    });
    assert_eq!(
        expression.evaluate(&ctx).unwrap(),
        ExpressionValue::Number(87_654_321.0)
    );
}

#[test]
fn instance_globals_read_the_instance_scope_and_fail_outside_it() {
    let expression = Expression::parse("instance * 10 + instance_count").unwrap();
    let mut ctx = test_context();
    let error = expression.evaluate(&ctx).unwrap_err().to_string();
    assert!(error.contains("`instance` is only available"), "{error}");
    ctx.instance = Some(crate::expr::InstanceScope { index: 3, count: 7 });
    assert_eq!(
        expression.evaluate(&ctx).unwrap(),
        ExpressionValue::Number(37.0)
    );
}

#[test]
fn evaluates_fractional_frame_and_time() {
    let ctx = ExpressionContext {
        frame: 1.5,
        ..test_context()
    };

    let expression = Expression::parse("vec2(frame * 2, time * 24)").unwrap();
    assert_eq!(
        expression.evaluate(&ctx).unwrap(),
        ExpressionValue::Vec2((3.0, 1.5))
    );
}

#[test]
fn subframe_offsets_frame_and_time() {
    let ctx = test_context().at_subframe(-0.25);

    let value = |source: &str| Expression::parse(source).unwrap().evaluate(&ctx).unwrap();
    assert_eq!(value("frame"), ExpressionValue::Number(47.75));
    assert_eq!(value("time"), ExpressionValue::Number(47.75 / 24.0));
}

#[test]
fn converts_supported_node_properties() {
    assert_eq!(7_i64.to_expression(), Some(ExpressionValue::Number(7.0)));
    assert_eq!(true.to_expression(), Some(ExpressionValue::Boolean(true)));
    assert_eq!(
        (10.0, 20.0).to_expression(),
        Some(ExpressionValue::Vec2((10.0, 20.0)))
    );
    assert_eq!(
        [255_u8, 0, 0, 255].to_expression(),
        Some(ExpressionValue::Color([1.0, 0.0, 0.0, 1.0]))
    );
}

#[test]
fn rejects_non_scalar_node_properties() {
    let graph = chained_text_nodes(1, "", "");
    let error = evaluate_in(&graph, "node(1, 'color')")
        .unwrap_err()
        .to_string();

    assert!(
        error.contains("property `color` on node 1 can't be read in an expression"),
        "{error}"
    );
}

#[test]
fn evaluates_builtins_and_components() {
    for (source, expected) in [
        ("lerp(10, 20, 0.25)", ExpressionValue::Number(12.5)),
        ("step(10, 20, 0.5)", ExpressionValue::Number(10.0)),
        ("step(10, 20, 1)", ExpressionValue::Number(20.0)),
        (
            "vec2(frame * 2, time + 3)",
            ExpressionValue::Vec2((96.0, 5.0)),
        ),
    ] {
        assert_eq!(evaluate(source, 48).unwrap(), expected, "{source}");
    }
}

fn number_at(source: &str, frame: u32) -> f64 {
    let ctx = ExpressionContext {
        frame: f64::from(frame),
        fps: 30.0,
        ..test_context()
    };
    match Expression::parse(source).unwrap().evaluate(&ctx).unwrap() {
        ExpressionValue::Number(value) => value,
        other => panic!("expected a number, got {other:?}"),
    }
}

#[test]
fn evaluates_issue_easing_examples() {
    let lerp = "lerp(1200, 540, out_expo(progress(0, 18)))";
    let tween = "tween(1200, 540, 0, 18, 'out_expo')";
    assert_eq!(number_at(lerp, 0), 1200.0);
    assert_eq!(number_at(lerp, 18), 540.0);
    assert_eq!(number_at(lerp, 40), 540.0);
    assert_eq!(number_at(lerp, 9), 1200.0 - 660.0 * (1.0 - 2f64.powi(-5)));
    for frame in [0, 3, 9, 17, 18, 30] {
        assert_eq!(number_at(lerp, frame), number_at(tween, frame));
    }
    assert_eq!(number_at("tween(0, 10, 4, 10)", 9), 5.0);
}

#[test]
fn unit_suffixes_convert_seconds_to_frames() {
    let source = "tween(0, 1, 1.5s, 300ms, 'out_expo')";
    assert_eq!(number_at(source, 45), 0.0);
    assert_eq!(number_at(source, 54), 1.0);
    assert_eq!(number_at(source, 48), 1.0 - 2f64.powf(-10.0 / 3.0));
    assert_eq!(number_at("2s + 500ms", 0), 75.0);

    let ExpressionError::Parse { details, .. } = Expression::parse("1.5px").unwrap_err() else {
        panic!("expected parse error");
    };
    assert!(details.contains("unknown unit `px`"), "{details}");
}

#[test]
fn progress_steps_when_the_range_is_empty() {
    assert_eq!(number_at("progress(10, 10)", 9), 0.0);
    assert_eq!(number_at("progress(10, 10)", 10), 1.0);
    assert_eq!(number_at("progress(10, 5)", 12), 1.0);
    assert_eq!(number_at("progress(10, 20)", 15), 0.5);
}

#[test]
fn spring_and_cubic_bezier_evaluate() {
    assert_eq!(number_at("spring((frame - 12) / fps)", 6), 0.0);
    assert!((number_at("spring((frame - 12) / fps, 170, 26)", 60) - 1.0).abs() < 1e-3);
    assert!((number_at("ease(cubic_bezier(vec2(0, 0), vec2(1, 1)), 0.25)", 0) - 0.25).abs() < 1e-6);
    let error = Expression::parse("spring(1, 0 - 1)")
        .unwrap()
        .evaluate(&test_context())
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("stiffness must be greater than 0")
    );
}

#[test]
fn noise_and_random_evaluate() {
    assert_eq!(number_at("random(7)", 0), number_at("random(0, 7)", 0));
    assert_ne!(number_at("random(frame)", 1), number_at("random(frame)", 2));
    assert_ne!(
        number_at("noise(1, time)", 3),
        number_at("noise(2, time)", 3)
    );
    assert!((-1.0..=1.0).contains(&number_at("noise(1, frame / 7)", 40)));
    let error = evaluate("noise(1, pow(10, 999))", 0)
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("arguments must be finite numbers"),
        "{error}"
    );
}

#[test]
fn rejects_bad_builtin_arguments_at_parse_time() {
    for (source, expected) in [
        ("vec2()", "`vec2`: expects 2 arguments"),
        ("vec2(1)", "`vec2`: expects 2 arguments"),
        ("vec2(1, 2, 3)", "`vec2`: expects 2 arguments"),
        ("tween(0, 1, 0, 10, 'bounce')", "unknown easing `bounce`"),
        ("tween(0, 1, 0, 10, frame)", "`ease` must be an easing"),
        ("tween(0, 1, 0)", "expects 4 to 5 arguments, got 3"),
        ("spring(1, 0)", "stiffness must be greater than 0"),
        ("spring(1, 170, -0.5 * 2)", ""),
    ] {
        match Expression::parse(source) {
            Err(ExpressionError::Parse { details, .. }) if !expected.is_empty() => {
                assert!(details.contains(expected), "{source}: {details}")
            }
            Ok(_) if expected.is_empty() => {}
            other => panic!("{source}: {other:?}"),
        }
    }
}

#[test]
fn keyframes_hold_outside_their_keys_and_interpolate_between() {
    let track = "keyframes(frame, key(10, 0), key(20, 100), key(40, 50))";
    for (frame, expected) in [
        (0, 0.0),
        (10, 0.0),
        (15, 50.0),
        (20, 100.0),
        (30, 75.0),
        (40, 50.0),
        (99, 50.0),
    ] {
        assert_eq!(number_at(track, frame), expected, "frame {frame}");
    }
    assert_eq!(number_at("keyframes(frame, key(5, 7))", 0), 7.0);
    assert_eq!(number_at("keyframes(frame, key(5, 7))", 9), 7.0);
}

#[test]
fn keys_ease_by_their_own_easing_or_the_default() {
    let track = "keyframes(frame, 'in_quad', key(0, 0), key(10, 100, 'linear'), key(20, 200))";
    assert_eq!(number_at(track, 5), 25.0);
    assert_eq!(number_at(track, 15), 150.0);
    assert_eq!(
        number_at("keyframes(frame, key(0, 0), key(10, 100))", 5),
        50.0
    );

    let hold = "keyframes(frame, key(0, 0, 'hold'), key(10, 100))";
    assert_eq!(number_at(hold, 9), 0.0);
    assert_eq!(number_at(hold, 10), 100.0);
    assert_eq!(number_at("tween(0, 10, 0, 10, 'hold')", 9), 0.0);
    assert_eq!(number_at("tween(0, 10, 0, 10, 'hold')", 10), 10.0);
}

#[test]
fn text_keys_switch_where_they_hold() {
    let text_at = |source: &str, frame: u32| match evaluate(source, frame) {
        Ok(ExpressionValue::String(text)) => text.to_string(),
        other => panic!("{source}: {other:?}"),
    };
    for track in [
        "keyframes(frame, 'hold', key(0, 'ten'), key(30, 'hundred'), key(60, '1,024'))",
        "keyframes(frame, key(0, 'ten', 'hold'), key(30, 'hundred', 'hold'), key(60, '1,024'))",
    ] {
        let at = |frame| text_at(track, frame);
        assert_eq!(
            (at(0), at(29), at(30)),
            ("ten".into(), "ten".into(), "hundred".into())
        );
        assert_eq!(at(59), "hundred");
    }
    assert_eq!(
        text_at(
            "keyframes(frame, 'hold', key(0, 'ten'), key(30, 'M 0 0'))",
            30
        ),
        "M 0 0"
    );
}

#[test]
fn text_keys_need_a_hold_to_switch() {
    for source in [
        "keyframes(frame, key(0, 'ten'), key(30, 'hundred'))",
        "keyframes(frame, key(0, 'ten', 'hold'), key(30, 'hundred', 'linear'), key(60, 'x'))",
        "keyframes(frame, key(0, 'ten', 'hold'), key(frame * 0 + 30, 'a', 'out_quad'), key(60, 'x'))",
    ] {
        let error = match Expression::parse(source) {
            Ok(expression) => expression
                .evaluate(&test_context())
                .unwrap_err()
                .to_string(),
            Err(error) => error.to_string(),
        };
        assert!(
            error.contains("text keys need the 'hold' easing"),
            "{source}: {error}"
        );
    }
}

#[test]
fn path_data_keys_still_interpolate_and_hold() {
    let path = "keyframes(frame, key(0, 'M 0 0 L 10 0'), key(10, 'M 0 0 L 20 0'))";
    assert_eq!(
        evaluate(path, 5).unwrap(),
        ExpressionValue::String("M 0 0 L 15 0".into())
    );
    let hold = "keyframes(frame, 'hold', key(0, 'M 0 0 L 10 0'), key(10, 'M 0 0 Q 1 1 2 2'))";
    assert_eq!(
        evaluate(hold, 5).unwrap(),
        ExpressionValue::String("M 0 0 L 10 0".into())
    );
}

#[test]
fn cubic_bezier_is_the_css_curve() {
    let close = |source: &str, expected: f64| {
        let value = number_at(source, 5);
        assert!((value - expected).abs() < 1e-3, "{source}: {value}");
    };
    // CSS `ease` at x = 0.5.
    let css_ease = "cubic_bezier(vec2(0.25, 0.1), vec2(0.25, 1))";
    close(&format!("ease({css_ease}, 0.5)"), 0.8024);
    close(&format!("tween(0, 1, 0, 10, {css_ease})"), 0.8024);
    close(
        &format!("keyframes(frame, key(0, 0, {css_ease}), key(10, 1))"),
        0.8024,
    );
    close("ease(cubic_bezier(vec2(0, 0), vec2(1, 1)), 0.25)", 0.25);
    assert_eq!(
        number_at("ease(cubic_bezier(vec2(0.42, 0), vec2(0.58, 1)), 1)", 0),
        1.0
    );
    assert_eq!(
        number_at("ease('out_expo', 0.5)", 0),
        number_at("out_expo(0.5)", 0)
    );
}

#[test]
fn vec2_and_colour_keys_interpolate_like_lerp() {
    assert_eq!(
        evaluate(
            "keyframes(frame, key(0, vec2(0, 10)), key(10, vec2(10, 30)))",
            5
        )
        .unwrap(),
        ExpressionValue::Vec2((5.0, 20.0))
    );
    let colour = evaluate("keyframes(frame, key(0, #ff000000), key(10, #0000ff))", 5);
    assert_eq!(
        colour.unwrap(),
        ExpressionValue::Color([0.0, 0.0, 1.0, 0.5])
    );
    assert_eq!(
        evaluate("keyframes(frame, key(0, #ff000000), key(10, #0000ff))", 5).unwrap(),
        evaluate("lerp(#ff000000, #0000ff, 0.5)", 0).unwrap()
    );
}

#[test]
fn keys_at_the_same_time_jump() {
    let track = "keyframes(frame, key(0, 0), key(10, 10), key(10, 50), key(20, 60))";
    assert_eq!(number_at(track, 9), 9.0);
    assert_eq!(number_at(track, 10), 50.0);
    assert_eq!(number_at(track, 15), 55.0);
}

#[test]
fn boolean_keys_are_numbers_everywhere_on_the_track() {
    let track = "keyframes(frame, key(10, false), key(20, true))";
    for (frame, expected) in [(0, 0.0), (15, 0.5), (20, 1.0)] {
        assert_eq!(
            evaluate(track, frame).unwrap(),
            ExpressionValue::Number(expected)
        );
    }
    // Frames and seconds aren't ordered until `fps` is known.
    assert!(Expression::parse("keyframes(frame, key(1s, 0), key(20, 1))").is_ok());
}

#[test]
fn rejects_bad_easings_and_keys_at_parse_time() {
    for (source, expected) in [
        (
            "cubic_bezier(0.4, 0, 0.2, 1, 0.5)",
            "`ease(cubic_bezier(vec2(x1, y1), vec2(x2, y2)), t)`",
        ),
        (
            "keyframes(frame, 'bounce', key(0, 0))",
            "unknown easing `bounce`",
        ),
        (
            "ease(cubic_bezier(vec2(1.2, 0), vec2(0, 1)), 0.5)",
            "must be in 0..=1",
        ),
        (
            "ease(cubic_bezier(vec2(0, 0), vec2(-0.1, 1)), 0.5)",
            "must be in 0..=1",
        ),
        (
            "keyframes(frame, key(10, 0), key(5, 1))",
            "a key at 5 follows one at 10",
        ),
        (
            "keyframes(frame, key(2s, 0), key(1.5s, 1))",
            "a key at 1.5s follows one at 2s",
        ),
        (
            "keyframes(frame, key(-1s, 0), key(-2s, 1))",
            "a key at -2s follows one at -1s",
        ),
        (
            "keyframes(frame, key(-100ms, 0), key(-200ms, 1))",
            "a key at -0.2s follows one at -0.1s",
        ),
        (
            "keyframes(frame, key(0, 0), key(10, vec2(1, 1)))",
            "same kind of value, got number and vec2",
        ),
        (
            "keyframes(frame, key(0, vec3(0, 0, 0)), key(10, vec2(0, 0)))",
            "same kind of value, got vec3 and vec2",
        ),
        ("keyframes(frame, 'linear')", "expects at least one key"),
        (
            "keyframes(frame, key(0, 'a'), key(10, 'b'))",
            "the key at 0 is malformed path data",
        ),
        (
            "keyframes(frame, 'hold', key(0, 'a'), key(10, 'b', 'linear'), key(20, 'c'))",
            "the key at 10 is malformed path data",
        ),
        (
            "abs(cubic_bezier(vec2(0, 0), vec2(1, 1)))",
            "`value` does not take an easing",
        ),
        (
            "cubic_bezier(vec2(0.4, 0), vec2(0.2, 1))",
            "the expression is an easing, which no param takes",
        ),
        ("key(0, 1)", "the expression is a key, which no param takes"),
    ] {
        match Expression::parse(source) {
            Err(ExpressionError::Parse { details, .. }) => {
                assert!(details.contains(expected), "{source}: {details}")
            }
            other => panic!("{source}: {other:?}"),
        }
    }
}

#[test]
fn checks_easings_and_keys_it_can_only_see_when_evaluating() {
    for (source, expected) in [
        (
            "if(frame > 0, cubic_bezier(vec2(0, 0), vec2(1, 1)), 'linear')",
            "the expression is an easing, which no param takes",
        ),
        (
            "ease(if(frame > 0, 'bounce', 'linear'), 0.5)",
            "unknown easing `bounce`",
        ),
        (
            "ease(cubic_bezier(vec2(frame, 0), vec2(0, 1)), 0.5)",
            "must be in 0..=1",
        ),
        (
            "keyframes(frame, key(frame, 0), key(5, 1))",
            "a key at 5 follows one at 48",
        ),
    ] {
        let error = evaluate(source, 48).unwrap_err().to_string();
        assert!(error.contains(expected), "{source}: {error}");
    }
}

#[test]
fn vec2_coerces_boolean_components() {
    assert_eq!(
        evaluate("vec2(true, false)", 0).unwrap(),
        ExpressionValue::Vec2((1.0, 0.0))
    );
}

#[test]
fn vec2_rejects_invalid_component_types() {
    for source in [
        "vec2(\"left\", 2)",
        "vec2(\"12.5\", 2)",
        "vec2(1, vec2(2, 3))",
    ] {
        let error = Expression::parse(source)
            .unwrap()
            .evaluate(&test_context())
            .expect_err("nonnumeric component must fail");

        assert_eq!(error.to_string(), "vec2 expects numeric arguments");
    }
}

#[test]
fn numeric_strings_are_not_numbers() {
    for (source, expected) in [
        ("'1' + 2", "cannot convert string `1` into a number"),
        ("'1' > 0", "cannot convert string `1` into a number"),
        ("-'1'", "cannot convert string `1` into a number"),
        ("abs('1')", "abs expects numeric arguments"),
        (
            "text_width('a', '32')",
            "text_width expects numeric arguments",
        ),
        (
            "lerp(lowercase('0'), '1', 0.5)",
            "strings interpolate only as path data, and `from` is malformed path data",
        ),
    ] {
        let error = evaluate(source, 0).unwrap_err().to_string();
        assert!(error.contains(expected), "{source}: {error}");
    }
    assert_eq!(
        evaluate("uppercase(1) == '1'", 0).unwrap(),
        ExpressionValue::Boolean(true)
    );
}

#[test]
fn format_writes_fixed_decimals_and_pads() {
    for (source, expected) in [
        ("format(4.3, 2)", "4.30"),
        ("format(4.285714, 2)", "4.29"),
        ("format(frame / 7, 2)", "6.43"),
        ("format(2.5)", "3"),
        ("format(-2.5)", "-3"),
        ("format(0.125, 2)", "0.13"),
        ("format(-0.125, 2)", "-0.13"),
        ("format(1.005, 2)", "1.01"),
        ("format(2.675, 2)", "2.68"),
        ("format(0.4)", "0"),
        ("format(999.5)", "1000"),
        ("format(9.999, 2)", "10.00"),
        ("format(0.999, 0)", "1"),
        ("format(12, 3)", "12.000"),
        ("format(0.5, 1)", "0.5"),
        ("format(pow(10, 21))", "1000000000000000000000"),
        ("format(0.1 + 0.2, 20)", "0.30000000000000004000"),
        ("format(true)", "1"),
        ("format(-0.4)", "0"),
        ("format(-0.001, 2)", "0.00"),
        ("format(-0)", "0"),
        ("format(-0, 2)", "0.00"),
        ("format(5, 0, 4)", "   5"),
        ("format(5, 0, 4, '0')", "0005"),
        ("format(-5, 0, 4, '0')", "-005"),
        ("format(-5, 0, 4)", "  -5"),
        ("format(-5, 0, 4, '*')", "**-5"),
        ("format(3.14159, 2, 8, '0')", "00003.14"),
        ("format(12345, 0, 3, '0')", "12345"),
        ("format(7, 0, 3, 'é')", "éé7"),
        ("format(7, 0, 3, '日')", "日日7"),
        ("format(7, 0, 0)", "7"),
    ] {
        assert_eq!(
            evaluate(source, 45).unwrap(),
            ExpressionValue::String(expected.into()),
            "{source}"
        );
    }
    let ExpressionValue::String(widest) = evaluate("format(1, 0, 64, '0')", 0).unwrap() else {
        panic!("expected a string");
    };
    assert_eq!(&*widest, format!("{}1", "0".repeat(63)));
    // The width counts characters, not bytes.
    let ExpressionValue::String(wide) = evaluate("format(1, 0, 5, '日')", 0).unwrap() else {
        panic!("expected a string");
    };
    assert_eq!(wide.chars().count(), 5);
    assert_eq!(wide.len(), 13);
}

#[test]
fn format_rejects_bad_arguments() {
    for (source, expected) in [
        ("format(pow(10, 999))", "`value` must be finite"),
        ("format(-pow(10, 999))", "`value` must be finite"),
        (
            "format(1, 21)",
            "`decimals` must be a whole number from 0 to 20",
        ),
        (
            "format(1, -1)",
            "`decimals` must be a whole number from 0 to 20",
        ),
        (
            "format(1, 1.5)",
            "`decimals` must be a whole number from 0 to 20",
        ),
        (
            "format(1, 0, 65)",
            "`width` must be a whole number from 0 to 64",
        ),
        (
            "format(1, 0, -1)",
            "`width` must be a whole number from 0 to 64",
        ),
        (
            "format(1, 0, 2.5)",
            "`width` must be a whole number from 0 to 64",
        ),
        (
            "format(1, 0, 4, '')",
            "`fill` must be a string of exactly one character",
        ),
        (
            "format(1, 0, 4, '00')",
            "`fill` must be a string of exactly one character",
        ),
        (
            "format(1, 0, 4, 0)",
            "`fill` must be a string of exactly one character",
        ),
        ("format('1')", "format expects numeric arguments"),
        ("format()", "expects 1 to 4 arguments, got 0"),
        ("format(1, 2, 3, ' ', 5)", "expects 1 to 4 arguments, got 5"),
    ] {
        let error = match Expression::parse(source) {
            Ok(expression) => expression
                .evaluate(&test_context())
                .unwrap_err()
                .to_string(),
            Err(error) => error.to_string(),
        };
        assert!(error.contains(expected), "{source}: {error}");
    }
    for source in ["format(0, 20)", "format(1, 0, 64)"] {
        assert!(evaluate(source, 0).is_ok(), "{source}");
    }
    // Literal arguments are refused before any frame is evaluated.
    for source in [
        "format(1, -1)",
        "format(1, 0, -1)",
        "format(1, 0, 4, 'ab')",
        "format(1, 0, 4, '')",
        "format(1, 0, 4, 0)",
    ] {
        assert!(Expression::parse(source).is_err(), "{source}");
    }
    // A fill only known at evaluation is checked then.
    let dynamic = evaluate("format(1, 0, 4, concat('a', 'b'))", 0).unwrap_err();
    assert!(dynamic.to_string().contains("`fill` must be"), "{dynamic}");
    let nan = evaluate("format(pow(-1, 0.5))", 0).unwrap_err();
    assert!(nan.to_string().contains("`value` must be finite"), "{nan}");
}

#[test]
fn concat_refuses_a_result_over_its_limit() {
    let half = "a".repeat(crate::expr::builtins::MAX_CONCAT_BYTES / 2);
    assert!(evaluate(&format!("concat('{half}', '{half}')"), 0).is_ok());
    let error = evaluate(&format!("concat('{half}', '{half}', 'a')"), 0).unwrap_err();
    assert!(error.to_string().contains("is longer than"), "{error}");
}

#[test]
fn concat_joins_strings_and_numbers_like_a_text_param() {
    for (source, expected) in [
        ("concat('a')", "a"),
        ("concat(3)", "3"),
        ("concat('x ', round(frame * 3.7))", "x 111"),
        ("concat('f ', frame / 7)", "f 4.285714285714286"),
        ("concat('a', 'b', 'c')", "abc"),
        ("concat('', '')", ""),
        ("concat('on: ', true, ' ', frame > 100)", "on: true false"),
        ("concat(uppercase('x'), 1.5, lowercase('Y'))", "X1.5y"),
        (
            "concat('frame ', format(frame, 0, 4, '0'), ' / 1200')",
            "frame 0030 / 1200",
        ),
    ] {
        assert_eq!(
            evaluate(source, 30).unwrap(),
            ExpressionValue::String(expected.into()),
            "{source}"
        );
    }
}

#[test]
fn concat_rejects_values_without_a_text_form() {
    for (source, expected) in [
        ("concat()", "expects at least 1 arguments, got 0"),
        (
            "concat('a', vec2(1, 2))",
            "`parts` must be a string or number, got vec2",
        ),
        (
            "concat(vec3(1, 2, 3))",
            "`parts` must be a string or number, got vec3",
        ),
        (
            "concat('a', #ff0000)",
            "`parts` must be a string or number, got color",
        ),
        ("concat('a', key(1, 2))", "does not take a key"),
        (
            "concat('a', cubic_bezier(vec2(0, 0), vec2(1, 1)))",
            "does not take an easing",
        ),
        ("concat(if(frame > 0, vec2(1, 2), vec2(3, 4)))", "got vec2"),
    ] {
        let error = match Expression::parse(source) {
            Ok(expression) => expression
                .evaluate(&test_context())
                .unwrap_err()
                .to_string(),
            Err(error) => error.to_string(),
        };
        assert!(error.contains(expected), "{source}: {error}");
    }
    // `+` stays numeric.
    let error = evaluate("'x ' + 1", 0).unwrap_err().to_string();
    assert!(
        error.contains("cannot convert string `x ` into a number"),
        "{error}"
    );
}

#[test]
fn concat_builds_path_data_from_numbers() {
    let ExpressionValue::Vec2(point) = evaluate(
        "path_point(concat('M0 0 L', format(frame * 2, 1), ' ', 0), 0.5)",
        50,
    )
    .unwrap() else {
        panic!("expected a point");
    };
    assert_eq!(point, (50.0, 0.0));
}

#[test]
fn only_exact_zero_is_zero() {
    let tiny = 10_f64.powf(-20.0);
    for (source, expected) in [
        ("1 / pow(10, -20)", ExpressionValue::Number(1.0 / tiny)),
        ("1 % pow(10, -20)", ExpressionValue::Number(1.0 % tiny)),
        ("mod(1, pow(10, -20))", ExpressionValue::Number(1.0 % tiny)),
        ("if(pow(10, -20), 1, 2)", ExpressionValue::Number(1.0)),
        ("!pow(10, -20)", ExpressionValue::Boolean(false)),
        ("if(pow(-1, 0.5), 1, 2)", ExpressionValue::Number(2.0)),
        ("if(-0, 1, 2)", ExpressionValue::Number(2.0)),
        (
            "1 / sin(3.141592653589793) > 1",
            ExpressionValue::Boolean(true),
        ),
    ] {
        assert_eq!(evaluate(source, 0).unwrap(), expected, "{source}");
    }
    for (source, expected) in [
        ("1 / 0", "division by zero"),
        ("1 / -0", "division by zero"),
        ("1 % 0", "modulo by zero"),
        ("mod(1, 0)", "mod divisor must be non-zero"),
    ] {
        let error = evaluate(source, 0).unwrap_err().to_string();
        assert!(error.contains(expected), "{source}: {error}");
    }
}

#[test]
fn math_helpers_evaluate() {
    for (source, expected) in [
        ("sign(-3)", -1.0),
        ("sign(0)", 0.0),
        ("sign(2.5)", 1.0),
        ("atan2(0, 1)", 0.0),
        ("atan2(1, 0)", std::f64::consts::FRAC_PI_2),
        ("atan2(0, -1)", std::f64::consts::PI),
        ("hypot(3, 4)", 5.0),
        ("hypot(0, 0)", 0.0),
        ("remap(5, 0, 10, 100, 200)", 150.0),
        ("remap(0, 0, 10, 100, 200)", 100.0),
        ("remap(10, 0, 10, 100, 200)", 200.0),
        ("remap(15, 0, 10, 100, 200)", 250.0),
        ("remap(5, 10, 0, 0, 1)", 0.5),
    ] {
        assert_eq!(
            evaluate(source, 0).unwrap(),
            ExpressionValue::Number(expected),
            "{source}"
        );
    }
    let error = evaluate("remap(1, 2, 2, 0, 1)", 0).unwrap_err().to_string();
    assert!(error.contains("from_min != from_max"), "{error}");
}

fn evaluate(source: &str, frame: u32) -> crate::Result<ExpressionValue> {
    let ctx = ExpressionContext {
        frame: f64::from(frame),
        ..test_context()
    };
    Expression::parse(source).unwrap().evaluate(&ctx)
}

#[test]
fn repeated_subtrees_stay_lazy_and_keep_their_errors() {
    let number = ExpressionValue::Number;
    let boolean = ExpressionValue::Boolean;
    for (source, frame, expected) in [
        (
            "min(1, max(0, (time - 0.5) / 0.5)) * min(1, max(0, (time - 0.5) / 0.5))",
            18,
            Ok(number(0.25)),
        ),
        (
            "if(frame > 48, 10 / (frame - 48), (frame - 48) * 2)",
            0,
            Ok(number(-96.0)),
        ),
        (
            "if(frame > 48, 10 / (frame - 48), (frame - 48) * 2)",
            48,
            Ok(number(0.0)),
        ),
        (
            "if(frame > 48, 10 / (frame - 48), (frame - 48) * 2)",
            58,
            Ok(number(1.0)),
        ),
        (
            "frame == 48 || 10 / (frame - 48) > 1",
            48,
            Ok(boolean(true)),
        ),
        (
            "frame == 48 || 10 / (frame - 48) > 1",
            58,
            Ok(boolean(false)),
        ),
        ("10 / (frame - 48) + 10 / (frame - 48)", 58, Ok(number(2.0))),
        (
            "10 / (frame - 48) + 10 / (frame - 48)",
            48,
            Err("division by zero"),
        ),
    ] {
        let result = evaluate(source, frame).map_err(|error| error.to_string());
        assert_eq!(
            result,
            expected.map_err(str::to_string),
            "{source} at {frame}"
        );
    }
}

#[test]
fn subtrees_that_differ_only_in_the_sign_of_zero_stay_apart() {
    let ExpressionValue::Vec2((negative, positive)) =
        evaluate("vec2(-(time * 0), time * 0)", 48).unwrap()
    else {
        panic!("expected vec2");
    };
    assert_eq!(negative.to_bits(), (-0.0_f64).to_bits());
    assert_eq!(positive.to_bits(), 0.0_f64.to_bits());
}

#[test]
fn vec2_arithmetic_is_componentwise() {
    for (source, expected) in [
        ("vec2(1, 2) + vec2(10, 20)", (11.0, 22.0)),
        ("vec2(1, 2) - vec2(10, 20)", (-9.0, -18.0)),
        ("-vec2(1, -2)", (-1.0, 2.0)),
        ("vec2(1, 2) * 3", (3.0, 6.0)),
        ("3 * vec2(1, 2)", (3.0, 6.0)),
        ("vec2(4, 8) / 2", (2.0, 4.0)),
        ("8 / vec2(4, 2)", (2.0, 4.0)),
        ("vec2(2, 3) * vec2(4, 5)", (8.0, 15.0)),
        ("vec2(8, 15) / vec2(4, 5)", (2.0, 3.0)),
        ("lerp(vec2(0, 10), vec2(10, 30), 0.25)", (2.5, 15.0)),
    ] {
        assert_eq!(
            evaluate(source, 0).unwrap(),
            ExpressionValue::Vec2(expected),
            "{source}"
        );
    }
}

#[test]
fn vec2_rejects_mismatched_arithmetic() {
    for (source, expected) in [
        ("vec2(1, 2) + 3", "cannot apply `+` to vec2 and number"),
        ("3 - vec2(1, 2)", "cannot apply `-` to number and vec2"),
        ("vec2(1, 2) % 2", "cannot apply `%` to vec2 and number"),
        (
            "vec2(1, 2) % vec2(1, 2)",
            "cannot apply `%` to vec2 and vec2",
        ),
        ("vec2(1, 2) / 0", "division by zero"),
        (
            "vec2(1, 2) > 1",
            "cannot convert vec2 `[1, 2]` into a number",
        ),
        (
            "lerp(vec2(1, 2), 3, 0.5)",
            "`lerp` expects `from` and `to` of the same type",
        ),
    ] {
        let details = evaluate(source, 0).unwrap_err().to_string();
        assert!(details.starts_with(expected), "{source}: {details}");
    }
}

#[test]
fn vec3_arithmetic_and_interpolation_are_componentwise() {
    for (source, expected) in [
        ("vec3(1, 2, 3) + vec3(10, 20, 30)", [11.0, 22.0, 33.0]),
        ("-vec3(1, -2, 0)", [-1.0, 2.0, -0.0]),
        ("2 * vec3(1, 2, 3) / vec3(1, 2, 4)", [2.0, 2.0, 1.5]),
        (
            "lerp(vec3(0, 10, 20), vec3(10, 30, 60), 0.25)",
            [2.5, 15.0, 30.0],
        ),
        (
            "tween(vec3(0, 0, 0), vec3(10, 20, 30), 0, 10)",
            [5.0, 10.0, 15.0],
        ),
        (
            "keyframes(frame, key(0, vec3(0, 0, 0)), key(10, vec3(10, 20, 30)))",
            [5.0, 10.0, 15.0],
        ),
    ] {
        assert_eq!(
            evaluate(source, 5).unwrap(),
            ExpressionValue::Vec3(expected),
            "{source}"
        );
    }
}

#[test]
fn vec3_rejects_mismatched_values() {
    for (source, expected) in [
        (
            "vec3(1, 2, 3) + vec2(1, 2)",
            "cannot apply `+` to vec3 and vec2",
        ),
        ("vec3(1, 2, 3) % 2", "cannot apply `%` to vec3 and number"),
        (
            "vec3(1, 2, 3) * vec2(1, 2)",
            "cannot convert vec3 `[1, 2, 3]` into a number",
        ),
        ("vec3(1, vec2(2, 3), 4)", "vec3 expects numeric arguments"),
        (
            "lerp(vec3(1, 2, 3), vec2(1, 2), 0.5)",
            "`lerp` expects `from` and `to` of the same type",
        ),
    ] {
        let details = evaluate(source, 0).unwrap_err().to_string();
        assert!(details.contains(expected), "{source}: {details}");
    }
}

#[test]
fn evaluates_issue_vec2_tween_sum() {
    let source = "vec2(960, 1200) \
            + tween(vec2(0, 0), vec2(0, -660), 0, 18, 'out_expo') \
            + tween(vec2(0, 0), vec2(-1360, 0), 90, 14, 'in_back')";
    for (frame, expected) in [
        (0, (960.0, 1200.0)),
        (18, (960.0, 540.0)),
        (90, (960.0, 540.0)),
        (104, (-400.0, 540.0)),
    ] {
        assert_eq!(
            evaluate(source, frame).unwrap(),
            ExpressionValue::Vec2(expected),
            "frame {frame}"
        );
    }
    let ExpressionValue::Vec2((x, _)) = evaluate(source, 93).unwrap() else {
        panic!("expected vec2");
    };
    assert!(x > 960.0, "in_back pulls back before leaving: {x}");
}

#[test]
fn colour_literals_constructors_and_tweens() {
    for (source, expected) in [
        ("#ff8000", [1.0, 128.0 / 255.0, 0.0, 1.0]),
        ("#FF800080", [1.0, 128.0 / 255.0, 0.0, 128.0 / 255.0]),
        ("rgb(255, 0, 51)", [1.0, 0.0, 0.2, 1.0]),
        ("rgba(0, 0, 0, 0)", [0.0; 4]),
        ("lerp(#000000, #ffffff, 0.5)", [0.5, 0.5, 0.5, 1.0]),
        ("tween(#ff0000, #0000ff, 40, 16)", [0.5, 0.0, 0.5, 1.0]),
        ("if(frame > 10, #00ff00, #ff0000)", [0.0, 1.0, 0.0, 1.0]),
    ] {
        assert_eq!(
            evaluate(source, 48).unwrap(),
            ExpressionValue::Color(expected),
            "{source}"
        );
    }
}

#[test]
fn colour_errors() {
    for (source, expected) in [
        ("#ff00", "invalid colour `#ff00`"),
        ("#gg0000", "invalid colour `#gg0000`"),
        (
            "rgb(256, 0, 0)",
            "colour components must be between 0 and 255",
        ),
        ("rgb(1, 2)", "expects 3 arguments"),
    ] {
        let ExpressionError::Parse { details, .. } = Expression::parse(source).unwrap_err() else {
            panic!("{source}: expected parse error");
        };
        assert!(details.contains(expected), "{source}: {details}");
    }
    for (source, expected) in [
        ("#ff0000 + #00ff00", "cannot apply `+` to color and color"),
        ("#ff0000 * 2", "cannot apply `*` to color and number"),
        ("-#ff0000", "cannot convert color `#ff0000ff` into a number"),
        (
            "rgb(frame * 10, 0, 0)",
            "colour components must be between 0 and 255",
        ),
        (
            "lerp(#ff0000, vec2(0, 0), 0.5)",
            "expects `from` and `to` of the same type",
        ),
    ] {
        let error = evaluate(source, 48).unwrap_err().to_string();
        assert!(error.contains(expected), "{source}: {error}");
    }
}

#[test]
fn text_measure_builtins_follow_the_nodes_text_properties() {
    let text_id = NodeId::new(8);
    let mut graph = Graph::new();
    graph.nodes.insert(
        text_id,
        NodeKind::Text(Text {
            id: text_id,
            params: crate::node::source::text::TextParamsDelegate {
                layout: crate::node::source::text::TextLayoutParamsDelegate {
                    content: crate::node::source::text_content::TextContentDelegate::Text(
                        crate::node::Deferred::Expr(
                            Expression::parse("\"Morning update\"").unwrap(),
                        ),
                    ),
                    font_family: crate::node::Deferred::Expr(
                        Expression::parse("\"Roboto\"").unwrap(),
                    ),
                    font_size: crate::node::Deferred::Expr(
                        Expression::parse("24 + frame").unwrap(),
                    ),
                    max_width: crate::node::Deferred::Expr(Expression::parse("120").unwrap()),
                    ..Default::default()
                },
                ..Default::default()
            },
        }),
    );
    let measure = |function: &str, frame: f64, with_graph: bool| {
        let ctx = ExpressionContext {
            frame,
            graph: with_graph.then_some(&graph),
            ..test_context()
        };
        let source = if with_graph {
            format!("{function}(node(8))")
        } else {
            let size = 24.0 + frame;
            format!("{function}(\"Morning update\", {size}, 120, \"Roboto\")")
        };
        match Expression::parse(&source).unwrap().evaluate(&ctx) {
            Ok(ExpressionValue::Number(value)) => value,
            other => panic!("{source}: {other:?}"),
        }
    };
    for function in ["text_width", "text_height"] {
        let (first, later) = (measure(function, 0.0, true), measure(function, 8.0, true));
        assert_ne!(first, later, "{function} ignores the animated font size");
        assert_eq!(first, measure(function, 0.0, false), "{function} at 0");
        assert_eq!(later, measure(function, 8.0, false), "{function} at 8");
    }

    for source in [
        "text_width(node(8), 32, 300, \"Roboto\")",
        "text_height(node(8), 32, 300, \"Roboto\")",
    ] {
        let error = Expression::parse(source).unwrap_err().to_string();
        assert!(error.contains("does not accept override"), "{error}");
    }
}

#[test]
fn text_measure_builtins_apply_font_features() {
    use crate::node::source::text_content::FontFeatures;

    let text_id = NodeId::new(8);
    let width = |features: &[(&[u8; 4], u32)]| {
        let features = FontFeatures(
            features
                .iter()
                .map(|(tag, value)| (**tag, *value))
                .collect(),
        );
        let mut graph = Graph::new();
        graph.nodes.insert(
            text_id,
            NodeKind::Text(Text {
                id: text_id,
                params: crate::node::source::text::TextParamsDelegate {
                    layout: crate::node::source::text::TextLayoutParamsDelegate {
                        content: crate::node::source::text_content::TextContentDelegate::Text(
                            crate::node::Deferred::value("1111".to_string()),
                        ),
                        font_size: crate::node::Deferred::value(32.0),
                        font_features: crate::node::Delegated::into_delegate(features),
                        ..Default::default()
                    },
                    ..Default::default()
                },
            }),
        );
        let ctx = ExpressionContext {
            graph: Some(&graph),
            ..test_context()
        };
        match Expression::parse("text_width(node(8))")
            .unwrap()
            .evaluate(&ctx)
        {
            Ok(ExpressionValue::Number(width)) => width,
            other => panic!("expected a width, got {other:?}"),
        }
    };
    assert!(width(&[(b"pnum", 1)]) < width(&[]));
}

#[test]
fn a_measured_string_takes_the_defaults_of_a_text_node() {
    let mut graph = Graph::new();
    graph.nodes.insert(
        NodeId::new(8),
        NodeKind::Text(Text {
            id: NodeId::new(8),
            params: crate::node::source::text::TextParamsDelegate {
                layout: crate::node::source::text::TextLayoutParamsDelegate {
                    content: crate::node::source::text_content::TextContentDelegate::Text(
                        crate::node::Deferred::value("Morning update".to_string()),
                    ),
                    ..Default::default()
                },
                ..Default::default()
            },
        }),
    );
    let ctx = ExpressionContext {
        graph: Some(&graph),
        ..test_context()
    };
    let measure = |source: &str| Expression::parse(source).unwrap().evaluate(&ctx).unwrap();

    assert_eq!(
        measure("text_width('Morning update')"),
        measure("text_width(node(8))")
    );
    assert_eq!(
        measure("text_height('Morning update')"),
        measure("text_height(node(8))")
    );
    assert_eq!(
        crate::node::source::text::TextLayoutParams::default().font_family,
        lumen_text::DEFAULT_FONT_FAMILY
    );
}

#[test]
fn span_colors_may_read_the_measurement_of_their_text() {
    let text_id = NodeId::new(8);
    let content = crate::node::source::text_content::TextContentDelegate::from_json_value(
        &serde_json::json!([{
            "text": "Morning",
            "color": "=if(text_width(node(8)) > 10, #ff0000, #0000ff)",
            "underline": { "paint": "=if(text_height(node(8)) > 10, #ff0000, #0000ff)" }
        }]),
        "content",
    )
    .unwrap();
    let mut graph = Graph::new();
    graph.nodes.insert(
        text_id,
        NodeKind::Text(Text {
            id: text_id,
            params: crate::node::source::text::TextParamsDelegate {
                layout: crate::node::source::text::TextLayoutParamsDelegate {
                    content,
                    ..Default::default()
                },
                ..Default::default()
            },
        }),
    );
    let ctx = ExpressionContext {
        graph: Some(&graph),
        ..test_context()
    };

    let width = Expression::parse("text_width(node(8))")
        .unwrap()
        .evaluate(&ctx);

    assert!(matches!(width, Ok(ExpressionValue::Number(value)) if value > 10.0));
}

#[test]
fn span_baseline_shifts_may_read_the_measurement_of_their_text() {
    let text_id = NodeId::new(8);
    let content = crate::node::source::text_content::TextContentDelegate::from_json_value(
        &serde_json::json!([{
            "text": "Morning",
            "baseline_shift": "=text_height(node(8)) / 2"
        }]),
        "content",
    )
    .unwrap();
    let mut graph = Graph::new();
    graph.nodes.insert(
        text_id,
        NodeKind::Text(Text {
            id: text_id,
            params: crate::node::source::text::TextParamsDelegate {
                layout: crate::node::source::text::TextLayoutParamsDelegate {
                    content,
                    ..Default::default()
                },
                ..Default::default()
            },
        }),
    );
    let ctx = ExpressionContext {
        graph: Some(&graph),
        ..test_context()
    };

    let height = Expression::parse("text_height(node(8))")
        .unwrap()
        .evaluate(&ctx);

    assert!(matches!(height, Ok(ExpressionValue::Number(value)) if value > 0.0));
}

#[test]
fn recursive_text_measurement_returns_error_instead_of_panicking() {
    let text_id = NodeId::new(8);
    let mut graph = Graph::new();
    graph.nodes.insert(
        text_id,
        NodeKind::Text(Text {
            id: text_id,
            params: crate::node::source::text::TextParamsDelegate {
                layout: crate::node::source::text::TextLayoutParamsDelegate {
                    content: crate::node::source::text_content::TextContentDelegate::Text(
                        crate::node::Deferred::Expr(
                            Expression::parse("text_width(node(8))").unwrap(),
                        ),
                    ),
                    ..Default::default()
                },
                ..Default::default()
            },
        }),
    );
    let ctx = ExpressionContext {
        graph: Some(&graph),
        ..test_context()
    };

    let error = Expression::parse("text_width(node(8))")
        .unwrap()
        .evaluate(&ctx)
        .expect_err("recursive text measurement must fail");

    assert!(error.to_string().contains("do they read each other?"));
}

/// Text nodes `1..=count` with the given `font_size` and `content` expressions, where
/// `{next}` stands for the id of the node after it. The last node has plain values.
fn chained_text_nodes(count: u64, font_size: &str, content: &str) -> Graph {
    let mut graph = Graph::new();
    for id in 1..=count {
        let mut params = crate::node::source::text::TextParamsDelegate::default();
        if id < count {
            let next = (id + 1).to_string();
            params.layout.font_size = crate::node::Deferred::Expr(
                Expression::parse(&font_size.replace("{next}", &next)).unwrap(),
            );
            params.layout.content = crate::node::source::text_content::TextContentDelegate::Text(
                crate::node::Deferred::Expr(
                    Expression::parse(&content.replace("{next}", &next)).unwrap(),
                ),
            );
        }
        graph.nodes.insert(
            NodeId::new(id),
            NodeKind::Text(Text {
                id: NodeId::new(id),
                params,
            }),
        );
    }
    graph
}

fn evaluate_in(graph: &Graph, source: &str) -> crate::Result<ExpressionValue> {
    let ctx = ExpressionContext {
        graph: Some(graph),
        ..test_context()
    };
    Expression::parse(source).unwrap().evaluate(&ctx)
}

#[test]
fn a_node_reading_itself_fails_instead_of_overflowing_the_stack() {
    let graph = chained_text_nodes(2, "node(1, 'font_size')", "'a'");
    let error = evaluate_in(&graph, "node(1, 'font_size')").expect_err("self read");

    assert!(error.to_string().contains("levels of evaluation"));
}

#[test]
fn nodes_reading_each_other_fail_and_leave_later_reads_working() {
    let mut graph = chained_text_nodes(3, "1", "'a'");
    for (id, other) in [(1, 2), (2, 1)] {
        let NodeKind::Text(text) = graph.nodes.get_mut(&NodeId::new(id)).unwrap() else {
            unreachable!()
        };
        text.params.layout.font_size = crate::node::Deferred::Expr(
            Expression::parse(&format!("node({other}, 'font_size')")).unwrap(),
        );
    }

    let error = evaluate_in(&graph, "node(1, 'font_size')").expect_err("loop");
    assert!(error.to_string().contains("levels of evaluation"));

    let after = evaluate_in(&graph, "node(3, 'font_size')").unwrap();
    assert!(matches!(after, ExpressionValue::Number(_)));
}

#[test]
fn a_shallow_fan_out_that_multiplies_work_is_cut_off() {
    let read = "node({next}, 'font_size')";
    let graph = chained_text_nodes(8, &[read; 8].join(" + "), "'a'");
    let error = evaluate_in(&graph, "node(1, 'font_size')").expect_err("fan-out");

    assert!(error.to_string().contains("more than 10000 evaluations"));
}

#[test]
fn fan_out_through_properties_and_measurements_shares_one_budget() {
    let font_size = ["node({next}, 'font_size')"; 5].join(" + ");
    let content = ["text_width(node({next}))"; 5].join(" + ");
    let graph = chained_text_nodes(8, &font_size, &content);
    let error = evaluate_in(&graph, "text_width(node(1)) + node(1, 'font_size')")
        .expect_err("mixed fan-out");

    assert!(error.to_string().contains("more than 10000 evaluations"));
    let after = evaluate_in(&graph, "node(8, 'font_size')").unwrap();
    assert!(matches!(after, ExpressionValue::Number(_)));
}

#[test]
fn wide_shallow_reads_stay_within_budget() {
    let read = "node({next}, 'font_size')";
    let graph = chained_text_nodes(8, &format!("{read} + {read}"), "'a'");

    let value = evaluate_in(&graph, "node(1, 'font_size')").unwrap();

    assert!(matches!(value, ExpressionValue::Number(_)));
}

fn on_worker_stack<T: Send + 'static>(run: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::Builder::new()
        .stack_size(2 << 20)
        .spawn(run)
        .unwrap()
        .join()
        .unwrap()
}

#[test]
fn a_self_read_in_a_deep_expression_fails_on_a_worker_stack() {
    let read = format!(
        "{}node(1, 'font_size'){}",
        "abs(".repeat(63),
        ")".repeat(63)
    );
    let graph = chained_text_nodes(2, &read, "'a'");

    let error = on_worker_stack(move || evaluate_in(&graph, "node(1, 'font_size')").unwrap_err());

    assert!(error.to_string().contains("levels of evaluation"));
}

#[test]
fn a_chain_of_deep_expressions_fails_on_a_worker_stack() {
    let read = format!(
        "{}node({{next}}, 'font_size'){}",
        "abs(".repeat(63),
        ")".repeat(63)
    );
    let graph = chained_text_nodes(8, &read, "'a'");

    let error = on_worker_stack(move || evaluate_in(&graph, "node(1, 'font_size')").unwrap_err());

    assert!(error.to_string().contains("levels of evaluation"));
}

#[test]
fn the_whole_budget_fits_a_worker_stack_in_the_largest_frames() {
    // Right-nested binaries are the deepest frames.
    let nest = |levels: usize, leaf: &str| {
        format!("{}{leaf}{}", "1 + (".repeat(levels), ")".repeat(levels))
    };
    let cost = |depth: u32| depth + EVALUATION_OVERHEAD;
    let nodes = 2;
    let property_levels = (MAX_EVALUATION_LEVELS - cost(1)) / nodes - cost(1);
    let measure_levels = (MAX_EVALUATION_LEVELS - cost(2)) / nodes - cost(3);
    let read = nest(property_levels as usize, "node({next}, 'font_size')");
    let measure = format!(
        "text_width({})",
        nest(measure_levels as usize, "text_width(node({next}))")
    );
    let graph = std::sync::Arc::new(chained_text_nodes(nodes as u64 + 1, &read, &measure));

    for source in ["node(1, 'font_size')", "text_width(node(1))"] {
        let graph = graph.clone();
        let value = on_worker_stack(move || evaluate_in(&graph, source));
        assert!(value.is_ok(), "{source}: {value:?}");
    }
}

#[test]
fn clamp_with_a_nan_bound_is_an_error_instead_of_a_panic() {
    for source in ["clamp(1, pow(-1, 0.5), 2)", "clamp(1, 0, pow(-1, 0.5))"] {
        let error = Expression::parse(source)
            .unwrap()
            .evaluate(&test_context())
            .expect_err("NaN bound");
        assert!(error.to_string().contains("clamp requires min <= max"));
    }
}

#[test]
fn copies_evaluated_together_give_what_each_expression_gives_alone() {
    let ripple = "sin(hypot(mod(instance, 4), 2) * 0.05 - frame * 0.2) * 6";
    let sources = [
        format!("vec3((mod(instance, 4) - 1.5) * 1.2, floor(instance / 4), {ripple})"),
        format!("vec3(1, 1, {ripple} + 7)"),
        "keyframes(frame, key(0, 0, 'out_expo'), key(60, 6))".to_string(),
        "if(instance == 3, 10 / (frame - 48), instance)".to_string(),
        "instance < 3 || glyph > 0".to_string(),
        "if(instance < 5, 1, 10 / (frame - 48)) + instance_count".to_string(),
        "frame == 48 || 10 / (frame - 48) > instance".to_string(),
        "1 / (frame - 48) + instance".to_string(),
        "node(1, 'font_size') * instance + text_width(node(1)) * 0".to_string(),
        "sin(node(1, 'font_size')) + instance + sin(node(1, 'font_size'))".to_string(),
        "hypot(text_width('WW', 16), 2) + instance_count + hypot(text_width('WW', 16), 2)"
            .to_string(),
    ];
    let expressions: Vec<Expression> = sources
        .iter()
        .map(|source| Expression::parse(source).unwrap())
        .collect();
    let graph = chained_text_nodes(1, "32", "");
    for frame in [47.0, 48.0] {
        let mut together = InstanceEvaluator::new(&expressions);
        for index in 0..7 {
            let ctx = ExpressionContext {
                frame,
                graph: Some(&graph),
                instance: Some(InstanceScope { index, count: 7 }),
                ..test_context()
            };
            for (at, (source, expression)) in sources.iter().zip(&expressions).enumerate() {
                assert_eq!(
                    together
                        .evaluate(at, &ctx)
                        .map_err(|error| error.to_string()),
                    expression.evaluate(&ctx).map_err(|error| error.to_string()),
                    "{source} for instance {index} at frame {frame}"
                );
            }
        }
    }
}

/// Serves the bundled Roboto and Inter together for `Roboto`, and nothing for any other family.
#[derive(Debug, Default)]
struct RobotoWithInter {
    session: crate::media::TextSession,
}

struct RobotoAndInter;

impl crate::media::FontResolver for RobotoAndInter {
    fn data(&self) -> Result<Vec<Vec<u8>>, crate::error::MediaError> {
        Ok(vec![
            lumen_text::ROBOTO_REGULAR_BYTES.to_vec(),
            include_bytes!("../../../text/assets/inter/InterVariable-Latin.ttf").to_vec(),
        ])
    }
}

impl crate::media::MediaStore for RobotoWithInter {
    fn resolve(
        &self,
        name: &str,
        kind: crate::media::AssetKind,
    ) -> Result<crate::media::ResolvedMedia, crate::error::MediaError> {
        if name != "Roboto" || kind != crate::media::AssetKind::Font {
            return Err(crate::error::MediaError::source_not_found(
                name,
                "not served",
            ));
        }
        Ok(crate::media::ResolvedMedia::new(
            crate::media::MediaIdentity::of_key("font-collection"),
            crate::media::MediaResolver::Font(Box::new(RobotoAndInter)),
        ))
    }

    fn text_session(&self) -> Option<&crate::media::TextSession> {
        Some(&self.session)
    }
}

#[test]
fn text_measured_before_a_font_loads_is_not_shared_with_text_measured_after() {
    let sources = [
        "vec3(text_width('Variable weight', 32, 0, 'Inter'), instance, frame)",
        "vec3(text_width('WWW', 32, 0, 'Roboto'), 0, 0)",
        "vec3(1, 1, text_width('Variable weight', 32, 0, 'Inter'))",
    ];
    let expressions: Vec<Expression> = sources
        .iter()
        .map(|source| Expression::parse(source).unwrap())
        .collect();
    let (together_fonts, alone_fonts) = (RobotoWithInter::default(), RobotoWithInter::default());
    let mut together = InstanceEvaluator::new(&expressions);
    for index in 0..2 {
        let ctx = ExpressionContext {
            instance: Some(InstanceScope { index, count: 2 }),
            ..test_context()
        };
        for (at, expression) in expressions.iter().enumerate() {
            let shared = together.evaluate(
                at,
                &ExpressionContext {
                    media: Some(&together_fonts),
                    ..ctx
                },
            );
            let alone = expression.evaluate(&ExpressionContext {
                media: Some(&alone_fonts),
                ..ctx
            });
            assert_eq!(
                shared.map_err(|error| error.to_string()),
                alone.map_err(|error| error.to_string()),
                "{} for instance {index}",
                sources[at]
            );
        }
    }
}

#[test]
fn node_reads_in_later_copies_count_against_the_evaluation_budget() {
    let font_size = ["node({next}, 'font_size')"; 5].join(" + ");
    let graph = chained_text_nodes(7, &font_size, "'x'");
    for source in [
        "vec3(if(instance == 0, 0, node(1, 'font_size')), node(1, 'font_size'), node(1, 'font_size'))",
        "vec3(if(instance == 0, 0, sin(node(1, 'font_size'))), sin(node(1, 'font_size')), sin(node(1, 'font_size')))",
    ] {
        let expression = Expression::parse(source).unwrap();
        let mut together = InstanceEvaluator::new([&expression]);
        for index in 0..2 {
            let ctx = ExpressionContext {
                graph: Some(&graph),
                instance: Some(InstanceScope { index, count: 2 }),
                ..test_context()
            };
            let alone = expression.evaluate(&ctx).map_err(|error| error.to_string());
            let shared = together
                .evaluate(0, &ctx)
                .map_err(|error| error.to_string());
            assert_eq!(shared, alone, "{source} for instance {index}");
            assert_eq!(
                alone.is_err_and(|error| error.contains("10000 evaluations")),
                index == 1,
                "{source} for instance {index}"
            );
        }
    }
}

#[test]
fn folded_constants_give_the_values_of_the_unfolded_expression() {
    let number = ExpressionValue::Number;
    for (source, frame, expected) in [
        ("(1190 + (-1 * 1260)) + frame", 0, number(-70.0)),
        ("(1190 + (-1 * 1260)) + frame", 30, number(-40.0)),
        (
            "keyframes(frame, key(0, 0), key(5 + 5, 5 * 4, 'hold'))",
            5,
            number(10.0),
        ),
        (
            "keyframes(frame, key(0, 0), key(5 + 5, 5 * 4, 'hold'))",
            10,
            number(20.0),
        ),
        (
            "keyframes(frame, 'hold', key(0, 'a'), key(10, 'b'))",
            9,
            ExpressionValue::String("a".into()),
        ),
        ("tween(0, 10 * 2, 0, 5 + 5)", 0, number(0.0)),
        ("tween(0, 10 * 2, 0, 5 + 5)", 5, number(10.0)),
        ("tween(0, 10 * 2, 0, 5 + 5)", 12, number(20.0)),
        ("if(1 > 0, 3, 4) + if(frame > 1, 10, 20)", 2, number(13.0)),
        (
            "vec2(atan2(0 * -1, -1), atan2(0, -1))",
            0,
            ExpressionValue::Vec2((-std::f64::consts::PI, std::f64::consts::PI)),
        ),
    ] {
        assert_eq!(evaluate(source, frame).unwrap(), expected, "{source}");
    }
}

#[test]
fn a_failing_constant_subtree_fails_only_where_it_is_evaluated() {
    assert!(Expression::parse("1 / 0").is_ok());
    let error = evaluate("1 / 0", 0).unwrap_err().to_string();
    assert!(error.contains("division by zero"), "{error}");
    let error = evaluate("1 / (0 * -1)", 0).unwrap_err().to_string();
    assert!(error.contains("division by zero"), "{error}");

    let source = "if(frame > 5, 1 / 0, 2)";
    assert_eq!(evaluate(source, 0).unwrap(), ExpressionValue::Number(2.0));
    let error = evaluate(source, 6).unwrap_err().to_string();
    assert!(error.contains("division by zero"), "{error}");
}

#[test]
fn a_constant_subtree_is_one_step() {
    let expression = Expression::parse("(1190 + (-1 * 1260)) + frame").unwrap();
    let plan = &expression.plan;
    let Op::Binary(lhs, BinaryOp::Add, _) = &plan.steps[plan.roots[0]].op else {
        panic!("expected a sum at the root");
    };
    let Op::Constant(index) = &plan.steps[*lhs].op else {
        panic!("expected a constant, got {:?}", plan.steps[*lhs].op);
    };
    assert_eq!(plan.constants[*index], ExpressionValue::Number(-70.0));
    assert_eq!(plan.steps[*lhs].memo, None);
}
