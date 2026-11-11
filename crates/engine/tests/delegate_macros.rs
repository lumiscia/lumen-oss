use lumen_engine::{
    expr::{Expression, ExpressionContext, ExpressionValue},
    node::{
        Deferred, DelegateEvalContext, NodeId, PropertyEval, PropertyRead,
        vector::paint::{
            GradientInterpolation, GradientPaint, GradientSpread, GradientStop, GradientUnits,
            Paint, PaintDelegate, PaintKind,
        },
    },
};

#[test]
fn property_eval_reads_marked_properties() {
    let node = lumen_engine::node::processing::exposure::Exposure::default();

    assert_eq!(
        node.get_property("contrast"),
        Some(PropertyRead::Value(Some(ExpressionValue::Number(1.0))))
    );
    assert!(node.get_property("source").is_none());
}

#[test]
fn paint_delegate_round_trips_generated_enum_shape() {
    let solid = Paint::SolidColor(1, 2, 3, 255);
    assert_eq!(
        PaintDelegate::from(solid.clone()).into_evaluated().unwrap(),
        solid
    );

    let gradient = Paint::Gradient(GradientPaint {
        kind: PaintKind::RadialGradient,
        units: GradientUnits::UserSpace,
        spread: GradientSpread::Reflect,
        interpolation: GradientInterpolation::LinearSrgb,
        start: [1.0, 2.0],
        end: [3.0, 4.0],
        center: [5.0, 6.0],
        radius: [7.0, 8.0],
        focal: Some([5.5, 6.5]),
        angle: 9.0,
        stops: vec![GradientStop {
            offset: 0.25,
            color: [10, 20, 30, 40],
        }],
    });
    assert_eq!(
        PaintDelegate::from(gradient.clone())
            .into_evaluated()
            .unwrap(),
        gradient
    );
}

#[test]
fn paint_delegate_evaluation_error_names_node_and_property_path() {
    let delegate = PaintDelegate::SolidColor(
        Deferred::Expr(Expression::parse("\"not a number\"").unwrap()),
        Deferred::value(2),
        Deferred::value(3),
        Deferred::value(255),
    );
    let expr = ExpressionContext::default();
    let ctx = DelegateEvalContext {
        node_id: NodeId::new(7),
        expr: &expr,
    };

    assert_eq!(
        ctx.eval_field("paint", &delegate).unwrap_err().to_string(),
        "node 7 `paint`: expected a number, got string `not a number`"
    );
}

#[test]
fn structural_hash_derives_outside_the_engine() {
    #[derive(lumen_macros::StructuralHash)]
    struct Pair {
        a: u32,
        b: bool,
    }

    let _ = Pair { a: 1, b: true };
}
