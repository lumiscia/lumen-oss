use lumen::node::{NodeCategory, NodeKind, NodeProperty, PropertyEval};

#[test]
fn node_schemas_are_derived_from_node_structs() {
    let schemas = NodeKind::schemas();

    assert_eq!(schemas.len(), 18);
    assert!(schemas.iter().any(|schema| schema.kind == "media_output"));
    assert!(schemas.iter().any(|schema| schema.kind == "text"));

    let solid = schemas
        .iter()
        .find(|schema| schema.kind == "solid_color")
        .unwrap();
    assert_eq!(solid.category, NodeCategory::Source);
    assert_eq!(solid.inputs.len(), 0);
    assert_eq!(
        solid
            .properties
            .iter()
            .map(|property| property.name)
            .collect::<Vec<_>>(),
        vec!["color", "width", "height"]
    );
    assert!(matches!(
        solid
            .default_properties
            .iter()
            .find(|(name, _)| *name == "color")
            .map(|(_, value)| value),
        Some(NodeProperty::Color([0, 0, 0, 255]))
    ));

    let merge = schemas
        .iter()
        .find(|schema| schema.kind == "merge")
        .unwrap();
    assert_eq!(merge.inputs.len(), 3);
    assert_eq!(merge.inputs[2].name, "mask");
    assert!(merge.inputs[2].optional);

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
            .any(|property| property.name == "position")
    );
}

#[test]
fn derived_property_eval_reads_marked_properties() {
    let node = lumen::node::processing::exposure::Exposure::default();

    assert!(matches!(
        node.get_property("contrast").unwrap(),
        Some(NodeProperty::Float(1.0))
    ));
    assert!(node.get_property("source").unwrap().is_none());
}
