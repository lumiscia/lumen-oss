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
