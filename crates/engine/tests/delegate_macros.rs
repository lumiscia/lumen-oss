use lumen_engine::{
    error::{LumenError, PropertyError},
    expr::{Expression, ExpressionContext},
    node::{
        Deferred, DelegateEvalContext, NodeId, PropertyEval, PropertyExpression, PropertyValue,
        vector::paint::{
            GradientInterpolation, GradientPaint, GradientSpread, GradientStop, GradientUnits,
            Paint, PaintDelegate, PaintKind,
        },
    },
};

#[test]
fn property_eval_reads_marked_properties() {
    let node = lumen_engine::node::processing::exposure::Exposure::default();

    assert!(matches!(
        node.get_property("contrast"),
        Some(PropertyExpression::Value(PropertyValue::Float(1.0)))
    ));
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
        property_path: "paint",
        expr: &expr,
    };

    let error = delegate.try_into_evaluated(&ctx).unwrap_err();
    let LumenError::Property(PropertyError::InvalidType {
        node_id,
        property_path,
        ..
    }) = error
    else {
        panic!("unexpected error: {error:?}");
    };
    assert_eq!(node_id, NodeId::new(7));
    assert_eq!(property_path, "paint");
}
