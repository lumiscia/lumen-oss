use super::*;
use crate::{
    graph::Graph,
    node::{Deferred, NodeId, NodeKind, source::text::Text},
    scope::PropValues,
};

const FPS: [f32; 4] = [30.0, 60.0, 23.976, 29.97];

/// Frames 0 to 300 in steps of a quarter.
fn sampled_frames() -> Vec<f64> {
    (0..=1200).map(|step| f64::from(step) * 0.25).collect()
}

/// Checks over `sampled_frames` that the held value is `evaluate`'s, that its frames hold the
/// frame, and that every sampled frame among them evaluates to the same bits.
fn check(source: &str, base: ExpressionContext<'_>) {
    let expression = Expression::parse(source).unwrap_or_else(|error| panic!("{source}: {error}"));
    let frames = sampled_frames();
    for fps in FPS {
        let at = |frame: f64| ExpressionContext { frame, fps, ..base };
        let plain: Vec<_> = frames
            .iter()
            .map(|&frame| expression.evaluate(&at(frame)))
            .collect();
        // The samples around each one that evaluate to its bits.
        let same = |a: usize, b: usize| match (&plain[a], &plain[b]) {
            (Ok(a), Ok(b)) => same_bits(a, b),
            _ => false,
        };
        let mut run_start = vec![0; frames.len()];
        for index in 1..frames.len() {
            run_start[index] = if same(index - 1, index) {
                run_start[index - 1]
            } else {
                index
            };
        }
        let mut run_end = vec![frames.len() - 1; frames.len()];
        for index in (0..frames.len() - 1).rev() {
            run_end[index] = if same(index, index + 1) {
                run_end[index + 1]
            } else {
                index
            };
        }
        for (index, &frame) in frames.iter().enumerate() {
            let context = format!("`{source}` at frame {frame}, {fps} fps");
            let held = match (expression.evaluate_held(&at(frame)), &plain[index]) {
                (Ok(held), Ok(plain)) => {
                    assert!(
                        same_bits(&held.value, plain),
                        "{context}: {held:?} != {plain:?}"
                    );
                    held
                }
                (Err(held), Err(plain)) => {
                    assert_eq!(held.to_string(), plain.to_string(), "{context}");
                    continue;
                }
                (held, plain) => panic!("{context}: {held:?} != {plain:?}"),
            };
            assert!(contains(held.frames, frame), "{context}: {:?}", held.frames);
            let inside: Vec<usize> = (0..frames.len())
                .filter(|&other| contains(held.frames, frames[other]))
                .collect();
            let (first, last) = (inside[0], inside[inside.len() - 1]);
            assert!(
                run_start[index] <= first && last <= run_end[index],
                "{context}: holds on {:?} but changes at frame {}",
                held.frames,
                if run_start[index] > first {
                    frames[run_start[index] - 1]
                } else {
                    frames[run_end[index] + 1]
                }
            );
        }
    }
}

fn base() -> ExpressionContext<'static> {
    ExpressionContext {
        width: 1920,
        height: 1080,
        ..Default::default()
    }
}

#[test]
fn held_values_match_evaluation_and_hold_where_they_say() {
    for source in [
        "frame",
        "time",
        "fps + width - height",
        "frame * 2 + 1",
        "frame * 1e-9",
        "frame / 3",
        "(frame + 1e12) - 1e12",
        "time * 0.1",
        "frame * 0",
        "frame * 1e308 * 10",
        "50 - frame > 20",
        "-frame + 50 >= 20.25",
        "frame % 30",
        "tween(0, 1, 30, 20)",
        "tween(0, 100, 30, 20, 'out_expo')",
        "tween(vec2(0, 60), vec2(10, 0), 12.5, 16, 'in_out_cubic')",
        "tween(#ff000000, #00ff00ff, 40, 0)",
        "tween(rgba(255, 0, 0, 255), rgba(0, 0, 255, 0), 100.3, 7.7, cubic_bezier(vec2(0.2, 0), vec2(0.4, 1)))",
        "tween('M 0 0 L 10 10', 'M 5 5 L 20 0', 50, 30, 'hold')",
        "tween(0, 1, 30, 20) + tween(5, 0, 90, 10, 'out_back')",
        "progress(10, 40)",
        "progress(40, 40)",
        "progress(40, 10)",
        "out_back(progress(20, 50))",
        "ease('in_quad', progress(60, 61))",
        "ease('out_expo', clamp((frame - 10) / 18, 0, 1))",
        "in_out_elastic(clamp(time - 2, 0, 1))",
        "clamp(frame - 36 + 1, 0, 1) * clamp(120 - frame, 0, 1)",
        "clamp(frame - 35.5, 0, 1) * clamp(120.25 - frame, 0, 1)",
        "clamp(time * 3 - 1.7, 0, 1)",
        "clamp(frame, 1, 0)",
        "floor(time / 12)",
        "floor(frame / 7.5) * 7.5",
        "ceil(frame / 10) - frame / 10",
        "round(time * 2)",
        "floor(-frame / 9)",
        "min(max((time - floor(time / 2) * 2 - 0.5) / 0.25, 0), 1)",
        "max(min(frame - 35, 1), 0)",
        "min(frame, 50)",
        "max(100 - frame, 20)",
        "min(frame, 10, 20)",
        "min(30, frame * 2)",
        "if(frame < 30, 1, 2)",
        "if(frame >= 100, vec2(1, 2), vec2(frame, 0))",
        "if(30 > frame, 'a', 'b')",
        "if(frame < 75, frame, 75) * 2",
        "frame <= 60.5 && frame > 10",
        "frame > 200 || frame < 10",
        "!(frame < 20)",
        "if(frame < 200, 1, 1 / 0)",
        "if(frame < 150.5, 2, clamp(frame, 1, 0))",
        "keyframes(frame, key(10, 0), key(20, 100), key(40, 50, 'hold'), key(60, 0))",
        "keyframes(time, 'hold', key(0.5, 'one'), key(1.5, 'two'), key(2.25, 'three'))",
        "keyframes(frame * 2, 'out_quad', key(20, vec2(0, 0)), key(80, vec2(10, 5)), key(80, vec2(20, 0)), key(150, vec2(0, 0), 'hold'), key(160, vec2(1, 1)))",
        "keyframes(frame, key(50, #ff0000), key(70, #0000ff))",
        "keyframes(100 - frame, key(10, 1), key(30, 2, 'hold'), key(50, 3))",
        "keyframes(frame, 'hold', key(40, 'M 0 0 L 1 1'), key(80, 'M 0 0 L 2 2'))",
        "noise(1, floor(frame / 10))",
        "random(floor(time * 4))",
        "sin(floor(frame / 30))",
        "format(floor(time), 0, 3, '0')",
        "concat('t', floor(frame / 50))",
        "lerp(0, 10, step(0, 1, frame / 100))",
    ] {
        check(source, base());
    }
}

#[test]
fn held_values_read_props() {
    let props: PropValues = [
        ("start".to_string(), ExpressionValue::Number(40.0)),
        ("to".to_string(), ExpressionValue::Number(5.0)),
    ]
    .into();
    let ctx = ExpressionContext {
        props: Some(&props),
        ..base()
    };
    for source in [
        "tween(0, prop('to'), prop('start'), 20)",
        "clamp(frame - prop('start'), 0, 1)",
        "if(frame < prop('start'), prop('to'), 0)",
    ] {
        check(source, ctx);
    }
}

/// Text nodes `1..=3` whose font sizes are `=tween(10, 20, 30, 10)`, `=node(1, 'font_size') * 2`
/// and 12.
fn reading_graph() -> Graph {
    let mut graph = Graph::new();
    for (id, font_size) in [
        (1, Some("tween(10, 20, 30, 10)")),
        (2, Some("node(1, 'font_size') * 2")),
        (3, None),
    ] {
        let mut params = crate::node::source::text::TextParamsDelegate::default();
        params.layout.font_size = match font_size {
            Some(source) => Deferred::Expr(Expression::parse(source).unwrap()),
            None => Deferred::Value(12.0),
        };
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

#[test]
fn held_values_read_other_nodes() {
    let graph = reading_graph();
    let ctx = ExpressionContext {
        graph: Some(&graph),
        ..base()
    };
    for source in [
        "node(1, 'font_size') + 1",
        "node(2, 'font_size')",
        "node(3, 'font_size') * 2",
        "clamp(frame - node(3, 'font_size'), 0, 1)",
        "if(node(1, 'font_size') > 15, 1, frame)",
        "node(4, 'font_size')",
    ] {
        check(source, ctx);
    }
    let held = Expression::parse("node(2, 'font_size')")
        .unwrap()
        .evaluate_held(&ExpressionContext {
            frame: 50.0,
            fps: 30.0,
            ..ctx
        })
        .unwrap();
    assert_eq!(held.frames, (40.0, f64::INFINITY));
}

fn frames_at(source: &str, frame: f64) -> (f64, f64) {
    let ctx = ExpressionContext {
        frame,
        fps: 30.0,
        ..base()
    };
    Expression::parse(source)
        .unwrap()
        .evaluate_held(&ctx)
        .unwrap()
        .frames
}

#[test]
fn held_values_hold_for_their_whole_step() {
    let tween = "tween(0, 1, 30, 20)";
    assert_eq!(frames_at(tween, 10.0), (f64::NEG_INFINITY, 30.0));
    assert_eq!(frames_at(tween, 80.0), (50.0, f64::INFINITY));
    assert_eq!(frames_at(tween, 40.0), (40.0, 40.0));

    let gate = "clamp(frame - 36 + 1, 0, 1) * clamp(120 - frame, 0, 1)";
    assert_eq!(frames_at(gate, 10.0), (f64::NEG_INFINITY, 35.0));
    assert_eq!(frames_at(gate, 60.0), (36.0, 119.0));
    assert_eq!(frames_at(gate, 200.0), (120.0, f64::INFINITY));

    let (start, end) = frames_at("floor(time / 12)", 400.0);
    assert!(
        (360.0..360.25).contains(&start) && (719.75..720.0).contains(&end),
        "{start}..{end}"
    );

    let (start, end) = frames_at("if(frame < 30, 1, 2)", 10.0);
    assert!(start == f64::NEG_INFINITY && (29.75..30.0).contains(&end));

    let keys = "keyframes(frame, key(10, 0), key(20, 100), key(40, 50, 'hold'), key(60, 0))";
    assert_eq!(frames_at(keys, 50.0).0, 40.0);
    assert!((59.75..60.0).contains(&frames_at(keys, 50.0).1));
}
