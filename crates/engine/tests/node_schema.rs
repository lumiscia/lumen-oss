#![cfg(feature = "metadata")]

use lumen_engine::{
    expr::Expression,
    node::{
        Deferred, NodeCategory, NodeKind, PropertyEval, PropertyExpression, PropertyValue,
        vector::paint::{
            GradientInterpolation, GradientPaint, GradientSpread, GradientUnits, Paint,
            PaintDelegate, PaintKind,
        },
    },
};

#[test]
fn node_schemas_are_derived_from_node_structs() {
    let schemas = NodeKind::schemas();

    assert_eq!(schemas.len(), 25);
    assert!(schemas.iter().any(|schema| schema.kind == "media_output"));
    assert!(schemas.iter().any(|schema| schema.kind == "text"));
    assert!(schemas.iter().any(|schema| schema.kind == "path"));

    let background = schemas
        .iter()
        .find(|schema| schema.kind == "background")
        .unwrap();
    assert_eq!(background.category, NodeCategory::Source);
    assert_eq!(background.inputs.len(), 0);
    assert_eq!(
        background
            .properties
            .iter()
            .map(|property| property.id)
            .collect::<Vec<_>>(),
        vec!["paint", "width", "height", "anti_alias"]
    );
    assert!(matches!(
        background
            .default_properties
            .iter()
            .find(|(name, _)| *name == "paint")
            .map(|(_, value)| value),
        Some(PropertyValue::Paint(_))
    ));

    let merge = schemas
        .iter()
        .find(|schema| schema.kind == "merge")
        .unwrap();
    assert_eq!(merge.name, "Merge");
    assert_eq!(merge.inputs.len(), 3);
    assert_eq!(merge.inputs[2].name, "mask");
    assert!(merge.inputs[2].optional);
    let opacity = merge
        .properties
        .iter()
        .find(|property| property.id == "opacity")
        .unwrap();
    assert_eq!(opacity.name, "Opacity");
    assert_eq!(opacity.constraints.min, Some(0.0));
    assert_eq!(opacity.constraints.max, Some(1.0));

    let raster_multimerge = schemas
        .iter()
        .find(|schema| schema.kind == "raster_multimerge")
        .unwrap();
    assert!(
        raster_multimerge
            .properties
            .iter()
            .any(|property| property.id == "blend_mode")
    );

    let shape = schemas
        .iter()
        .find(|schema| schema.kind == "shape")
        .unwrap();
    assert_eq!(shape.category, NodeCategory::Vector);
    assert_eq!(shape.inputs.len(), 0);
    assert!(
        shape
            .properties
            .iter()
            .any(|property| property.id == "position")
    );
}

#[test]
fn derived_property_eval_reads_marked_properties() {
    let node = lumen_engine::node::processing::exposure::Exposure::default();

    assert!(matches!(
        node.get_property("contrast").unwrap(),
        Some(PropertyExpression::Value(PropertyValue::Float(1.0)))
    ));
    assert!(node.get_property("source").unwrap().is_none());
}

#[test]
fn paint_delegate_round_trips_generated_enum_shape() {
    let solid = Paint::SolidColor(1, 2, 3, 255);
    assert_eq!(
        PaintDelegate::from(solid.clone()).into_evaluated().unwrap(),
        solid
    );

    let gradient = Paint::Gradient(GradientPaint {
        kind: PaintKind::LinearGradient,
        units: GradientUnits::ObjectBoundingBox,
        spread: GradientSpread::Pad,
        interpolation: GradientInterpolation::Srgb,
        start: [0.0, 0.0],
        end: [1.0, 1.0],
        center: [0.5, 0.5],
        radius: [0.5, 0.5],
        angle: 0.0,
        stops: Vec::new(),
    });
    assert_eq!(
        PaintDelegate::from(gradient.clone())
            .into_evaluated()
            .unwrap(),
        gradient
    );
}

#[test]
fn paint_delegate_into_evaluated_reports_expression_errors() {
    let delegate = PaintDelegate::SolidColor(
        Deferred::Expr(Expression::parse("\"not a number\"").unwrap()),
        Deferred::value(2),
        Deferred::value(3),
        Deferred::value(255),
    );

    assert!(delegate.into_evaluated().is_err());
}
