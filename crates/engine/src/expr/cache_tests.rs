use super::*;
use crate::{
    expr::{GlyphScope, InstanceScope, evaluation_count},
    graph::Graph,
    node::{Deferred, NodeId, NodeKind, source::text::Text},
};

fn base() -> ExpressionContext<'static> {
    ExpressionContext {
        fps: 30.0,
        width: 64,
        height: 64,
        ..ExpressionContext::default()
    }
}

/// A graph whose text node 1 has the font size `source`.
fn graph_with_font_size(source: &str) -> Graph {
    let mut graph = Graph::new();
    let mut params = crate::node::source::text::TextParamsDelegate::default();
    params.layout.font_size = Deferred::Expr(Expression::parse(source).unwrap());
    graph.nodes.insert(
        NodeId::new(1),
        NodeKind::Text(Text {
            id: NodeId::new(1),
            params,
        }),
    );
    graph
}

/// `expression` in `ctx`, which must be the value it has without held values, and how many
/// evaluations that started.
fn evaluate(expression: &Expression, ctx: &ExpressionContext<'_>) -> (ExpressionValue, u64) {
    let before = evaluation_count();
    let value = expression.evaluate(ctx).unwrap();
    let started = evaluation_count() - before;
    let fresh = expression
        .evaluate(&ExpressionContext { held: None, ..*ctx })
        .unwrap();
    assert!(same_value(&value, &fresh), "frame {}", ctx.frame);
    (value, started)
}

#[test]
fn an_outermost_evaluation_reuses_the_value_while_it_holds() {
    let cache = HeldCache::default();
    let props = |to: f64| {
        Arc::new(PropValues::from([(
            "to".to_string(),
            ExpressionValue::Number(to),
        )]))
    };
    let (first, equal, other) = (props(10.0), props(10.0), props(20.0));
    // 0 up to frame 20, then 20 frames of motion, then `to` from frame 40.
    let expression = Expression::parse("tween(0, prop('to'), 20, 20)").unwrap();
    let at = |frame: f64, props: &Arc<PropValues>| {
        let ctx = ExpressionContext {
            frame,
            props: Some(props),
            held: Some(cache.scope(SceneId::Root, 0, Some(props))),
            ..base()
        };
        evaluate(&expression, &ctx).1
    };
    assert_eq!(at(5.0, &first), 1);
    assert_eq!(at(12.0, &first), 0);
    assert_eq!(at(30.0, &first), 1);
    // Equal props at another address are the same props.
    assert_eq!(at(12.0, &equal), 0);
    assert_eq!(at(50.0, &first), 1);
    assert_eq!(at(50.0, &other), 1);

    let scoped = ExpressionContext {
        frame: 12.0,
        props: Some(&first),
        held: Some(cache.scope(SceneId::Root, 0, Some(&first))),
        ..base()
    };
    for ctx in [
        ExpressionContext {
            glyph: Some(GlyphScope {
                item: lumen_text::TextItem::default(),
                counts: lumen_text::TextItem::default(),
            }),
            ..scoped
        },
        ExpressionContext {
            instance: Some(InstanceScope { index: 0, count: 1 }),
            ..scoped
        },
    ] {
        assert_eq!(evaluate(&expression, &ctx).1, 1);
    }
}

#[test]
fn a_node_read_is_evaluated_inside_its_reader() {
    let graph = graph_with_font_size("tween(10, 20, 30, 10)");
    let Some(NodeKind::Text(text)) = graph.nodes.get(&NodeId::new(1)) else {
        unreachable!()
    };
    let Deferred::Expr(read) = &text.params.layout.font_size else {
        unreachable!()
    };
    let reader = Expression::parse("node(1, 'font_size') + 1").unwrap();
    let cache = HeldCache::default();
    let ctx = ExpressionContext {
        frame: 5.0,
        graph: Some(&graph),
        held: Some(cache.scope(SceneId::Root, 0, None)),
        ..base()
    };
    assert_eq!(evaluate(read, &ctx).1, 1);
    // The read inside neither reuses the value kept for it nor keeps one.
    assert_eq!(evaluate(&reader, &ctx), (ExpressionValue::Number(11.0), 2));
    assert_eq!(
        evaluate(&reader, &ExpressionContext { frame: 6.0, ..ctx }).1,
        0
    );
    assert_eq!(
        evaluate(read, &ExpressionContext { frame: 6.0, ..ctx }).1,
        0
    );
}

#[test]
fn a_value_that_measured_text_is_kept_for_its_own_frame_only() {
    let cache = HeldCache::default();
    let measuring = Expression::parse("text_width('WW', 16)").unwrap();
    let holding = Expression::parse("tween(0, 1, 30, 20)").unwrap();
    let at = |expression: &Expression| {
        let ctx = ExpressionContext {
            frame: 5.0,
            held: Some(cache.scope(SceneId::Root, 0, None)),
            ..base()
        };
        evaluate(expression, &ctx).1
    };
    assert_eq!((at(&measuring), at(&measuring)), (1, 0));
    assert_eq!((at(&holding), at(&holding)), (1, 0));

    let mut cache = cache;
    cache.begin_frame();
    let at = |expression: &Expression| {
        let ctx = ExpressionContext {
            frame: 5.0,
            held: Some(cache.scope(SceneId::Root, 0, None)),
            ..base()
        };
        evaluate(expression, &ctx).1
    };
    assert_eq!(at(&measuring), 1);
    assert_eq!(at(&holding), 0);
}

#[test]
fn a_value_that_changed_last_frame_is_the_value_evaluating_gives() {
    let cache = HeldCache::default();
    let at = |expression: &Expression, frame: f64| {
        let ctx = ExpressionContext {
            frame,
            held: Some(cache.scope(SceneId::Root, 0, None)),
            ..base()
        };
        evaluate(expression, &ctx).1
    };
    let changing = Expression::parse("sin(frame)").unwrap();
    for frame in 0..8 {
        assert_eq!(at(&changing, f64::from(frame)), 1);
    }

    let settling = Expression::parse("min(frame, 3)").unwrap();
    for frame in 0..=4 {
        at(&settling, f64::from(frame));
    }
    for frame in 5..8 {
        assert_eq!(at(&settling, f64::from(frame)), 0);
    }
}

#[test]
fn a_value_that_repeats_is_walked_again_less_and_less_often() {
    let started = |source: &str, expected: &dyn Fn(u32) -> f64| {
        let mut cache = HeldCache::default();
        let expression = Expression::parse(source).unwrap();
        let mut started = Vec::new();
        for frame in 0..16 {
            let ctx = ExpressionContext {
                frame: f64::from(frame),
                held: Some(cache.scope(SceneId::Root, 0, None)),
                ..base()
            };
            let (value, count) = evaluate(&expression, &ctx);
            assert_eq!(value, ExpressionValue::Number(expected(frame)), "{source}");
            started.push(count);
            cache.begin_frame();
        }
        started
    };
    // It never holds for more than a frame: the walks are 1, 2, 4 and 8 frames apart.
    assert_eq!(
        started("if(frame % 100 < 50, 1, 0)", &|_| 1.0),
        [1, 2, 1, 2, 1, 1, 2, 1, 1, 1, 1, 2, 1, 1, 1, 1]
    );
    // The frame its edge reaches the bound on gives the value it then holds, for that frame only:
    // the next walk finds the rest.
    assert_eq!(
        started("clamp(frame - 4 + 1, 0, 1)", &|frame| f64::from(u8::from(
            frame >= 4
        )))[4..8],
        [1, 2, 0, 0]
    );
}

#[test]
fn the_values_kept_for_one_expression_are_capped() {
    let cache = HeldCache::default();
    let changing = Expression::parse("sin(frame)").unwrap();
    let at = |frame: u32| {
        let ctx = ExpressionContext {
            frame: f64::from(frame),
            held: Some(cache.scope(SceneId::Root, 0, None)),
            ..base()
        };
        evaluate(&changing, &ctx).1
    };
    for frame in 0..20 {
        assert_eq!(at(frame), 1);
    }
    assert_eq!(at(19), 0);
    let kept: Vec<_> = cache
        .entries
        .borrow()
        .values()
        .map(|kept| kept.held.len())
        .collect();
    assert_eq!(kept, [MAX_HELD_PER_EXPRESSION]);
}
