//! Expression parsing, evaluation, display, and compilation tests.
//!
//! Tests cover edge cases in the expression parser (operator precedence,
//! nested parentheses, malformed input), the evaluator (division by zero,
//! unresolved refs, chained operations), the Display impl for round-trip
//! serialization, and compile-time expression resolution (forward refs,
//! circular deps, canvas refs).

use lumen::{
    BinOp, Canvas, Clip, ClipAnimation, ClipContent, ColorRgba, ExprEvalCtx, ExprEvalError,
    ExprParseError, ExprProp, ExprRef, Layer, LayerItem, ParsedExpr, Project, Scalar, TextClip,
    Timeline, Transform, UnaryOp, compile::CompileError, compile_project,
    compile_project_with_scale, eval_expr, parse_expr, time::Rational,
};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Parse edge cases
// ---------------------------------------------------------------------------

#[test]
fn parse_nested_parentheses() {
    let parsed = parse_expr("((1 + 2))").expect("parse");
    // ((1+2)) should be BinOp(Add, 1, 2)
    assert_eq!(eval_with_empty(&parsed), 3.0);
}

#[test]
fn parse_deeply_nested() {
    let parsed = parse_expr("(((canvas.width)))").expect("parse");
    assert!(matches!(parsed, ParsedExpr::Ref(_)));
}

#[test]
fn parse_subtraction_chain() {
    let parsed = parse_expr("10 - 3 - 2").expect("parse");
    // Left-associative: (10 - 3) - 2 = 5
    assert_eq!(eval_with_empty(&parsed), 5.0);
}

#[test]
fn parse_division_chain() {
    let parsed = parse_expr("100 / 5 / 4").expect("parse");
    // Left-associative: (100 / 5) / 4 = 5
    assert_eq!(eval_with_empty(&parsed), 5.0);
}

#[test]
fn parse_mixed_precedence() {
    let parsed = parse_expr("2 + 3 * 4 - 1").expect("parse");
    // 2 + (3*4) - 1 = 13
    assert_eq!(eval_with_empty(&parsed), 13.0);
}

#[test]
fn parse_unary_plus() {
    let parsed = parse_expr("+42").expect("parse");
    assert_eq!(eval_with_empty(&parsed), 42.0);
}

#[test]
fn parse_double_unary_minus() {
    let parsed = parse_expr("--5").expect("parse");
    // -(-5) = 5
    assert_eq!(eval_with_empty(&parsed), 5.0);
}

#[test]
fn parse_unary_minus_in_expr() {
    let parsed = parse_expr("10 + -3").expect("parse");
    assert_eq!(eval_with_empty(&parsed), 7.0);
}

#[test]
fn parse_whitespace_handling() {
    let parsed = parse_expr("   1   +   2   ").expect("parse");
    assert_eq!(eval_with_empty(&parsed), 3.0);
}

#[test]
fn parse_rejects_empty() {
    assert_eq!(parse_expr(""), Err(ExprParseError::Empty));
    assert_eq!(parse_expr("   "), Err(ExprParseError::Empty));
}

#[test]
fn parse_rejects_trailing_operator() {
    assert!(parse_expr("1 +").is_err());
}

#[test]
fn parse_rejects_leading_operator() {
    // * is not a unary operator
    assert!(parse_expr("* 1").is_err());
}

#[test]
fn parse_rejects_double_operator() {
    assert!(parse_expr("1 ++ 2").is_ok()); // + is unary on 2
    assert!(parse_expr("1 ** 2").is_err()); // * is not unary
}

#[test]
fn parse_rejects_unknown_property() {
    let err = parse_expr("canvas.z").unwrap_err();
    assert!(matches!(err, ExprParseError::UnknownProperty(_)));
}

#[test]
fn parse_rejects_missing_property() {
    let err = parse_expr("canvas.").unwrap_err();
    // After dot with no valid ident, should be malformed or unknown property
    assert!(
        matches!(err, ExprParseError::Malformed(_))
            || matches!(err, ExprParseError::UnknownProperty(_))
    );
}

#[test]
fn parse_all_properties() {
    for prop in &["width", "height", "x", "y"] {
        let parsed = parse_expr(&format!("node.{prop}")).expect(prop);
        assert!(matches!(parsed, ParsedExpr::Ref(_)));
    }
}

#[test]
fn parse_underscore_identifiers() {
    let parsed = parse_expr("my_clip_123.width + _private.height").expect("parse");
    assert!(matches!(parsed, ParsedExpr::BinOp { op: BinOp::Add, .. }));
}

#[test]
fn parse_scientific_notation_variants() {
    assert_eq!(eval_with_empty(&parse_expr("1e2").unwrap()), 100.0);
    assert_eq!(eval_with_empty(&parse_expr("1.5e2").unwrap()), 150.0);
    assert_eq!(eval_with_empty(&parse_expr("1e-1").unwrap()), 0.1);
}

// ---------------------------------------------------------------------------
// Eval edge cases
// ---------------------------------------------------------------------------

struct MapCtx(HashMap<(String, ExprProp), f32>);

impl ExprEvalCtx for MapCtx {
    fn resolve(&self, target: &str, property: ExprProp) -> Option<f32> {
        self.0.get(&(target.to_string(), property)).copied()
    }
}

fn eval_with_empty(expr: &ParsedExpr) -> f32 {
    let ctx = MapCtx(HashMap::new());
    eval_expr(expr, &ctx).expect("eval")
}

fn eval_with_ctx(expr_str: &str, ctx: &dyn ExprEvalCtx) -> Result<f32, ExprEvalError> {
    let parsed = parse_expr(expr_str).expect("parse");
    eval_expr(&parsed, ctx)
}

#[test]
fn eval_zero_division() {
    let parsed = parse_expr("10 / 0").expect("parse");
    let ctx = MapCtx(HashMap::new());
    assert_eq!(eval_expr(&parsed, &ctx), Err(ExprEvalError::DivisionByZero));
}

#[test]
fn eval_indirect_zero_division() {
    let parsed = parse_expr("10 / (5 - 5)").expect("parse");
    let ctx = MapCtx(HashMap::new());
    assert_eq!(eval_expr(&parsed, &ctx), Err(ExprEvalError::DivisionByZero));
}

#[test]
fn eval_unresolved_ref() {
    let parsed = parse_expr("ghost.width").expect("parse");
    let ctx = MapCtx(HashMap::new());
    let err = eval_expr(&parsed, &ctx).unwrap_err();
    assert!(matches!(err, ExprEvalError::UnresolvedRef { .. }));
}

#[test]
fn eval_complex_expression() {
    let mut map = HashMap::new();
    map.insert(("a".to_string(), ExprProp::Width), 100.0);
    map.insert(("a".to_string(), ExprProp::X), 20.0);
    map.insert(("b".to_string(), ExprProp::Height), 50.0);
    let ctx = MapCtx(map);

    // (a.x + a.width) * b.height / 2
    let result = eval_with_ctx("(a.x + a.width) * b.height / 2", &ctx).expect("eval");
    assert_eq!(result, 3000.0); // (20+100) * 50 / 2 = 3000
}

#[test]
fn eval_unary_minus_ref() {
    let mut map = HashMap::new();
    map.insert(("node".to_string(), ExprProp::Y), 30.0);
    let ctx = MapCtx(map);

    let result = eval_with_ctx("-node.y", &ctx).expect("eval");
    assert_eq!(result, -30.0);
}

#[test]
fn eval_multiplication_by_zero() {
    let parsed = parse_expr("999 * 0").expect("parse");
    assert_eq!(eval_with_empty(&parsed), 0.0);
}

// ---------------------------------------------------------------------------
// Display round-trip
// ---------------------------------------------------------------------------

#[test]
fn display_literal() {
    let expr = ParsedExpr::Literal(42.0);
    let s = expr.to_string();
    let reparsed = parse_expr(&s).expect("reparse");
    assert_eq!(eval_with_empty(&reparsed), 42.0);
}

#[test]
fn display_ref() {
    let expr = ParsedExpr::Ref(ExprRef {
        target: "canvas".to_string(),
        property: ExprProp::Width,
    });
    assert_eq!(expr.to_string(), "canvas.width");
}

#[test]
fn display_binary_op() {
    let expr = ParsedExpr::BinOp {
        op: BinOp::Add,
        lhs: Box::new(ParsedExpr::Literal(1.0)),
        rhs: Box::new(ParsedExpr::Literal(2.0)),
    };
    let s = expr.to_string();
    let reparsed = parse_expr(&s).expect("reparse");
    assert_eq!(eval_with_empty(&reparsed), 3.0);
}

#[test]
fn display_complex_expr_roundtrip() {
    // Build: (a.width + 10) * 2
    let expr = ParsedExpr::BinOp {
        op: BinOp::Mul,
        lhs: Box::new(ParsedExpr::BinOp {
            op: BinOp::Add,
            lhs: Box::new(ParsedExpr::Ref(ExprRef {
                target: "a".to_string(),
                property: ExprProp::Width,
            })),
            rhs: Box::new(ParsedExpr::Literal(10.0)),
        }),
        rhs: Box::new(ParsedExpr::Literal(2.0)),
    };
    let s = expr.to_string();
    let reparsed = parse_expr(&s).expect("reparse");

    let mut map = HashMap::new();
    map.insert(("a".to_string(), ExprProp::Width), 100.0);
    let ctx = MapCtx(map);

    let original_result = eval_expr(&expr, &ctx).expect("eval original");
    let reparsed_result = eval_expr(&reparsed, &ctx).expect("eval reparsed");
    assert_eq!(original_result, reparsed_result);
    assert_eq!(original_result, 220.0);
}

#[test]
fn display_unary_minus_roundtrip() {
    let expr = ParsedExpr::UnaryOp {
        op: UnaryOp::Minus,
        expr: Box::new(ParsedExpr::Literal(5.0)),
    };
    let s = expr.to_string();
    let reparsed = parse_expr(&s).expect("reparse");
    assert_eq!(eval_with_empty(&reparsed), -5.0);
}

#[test]
fn display_all_prop_names() {
    for (prop, name) in [
        (ExprProp::Width, "width"),
        (ExprProp::Height, "height"),
        (ExprProp::X, "x"),
        (ExprProp::Y, "y"),
    ] {
        assert_eq!(format!("{prop}"), name);
    }
}

// ---------------------------------------------------------------------------
// Compile-time expression resolution
// ---------------------------------------------------------------------------

fn text_clip(id: &str, transform: Transform, animation: ClipAnimation) -> Clip {
    Clip {
        id: id.to_string(),
        start_frame: 0,
        duration_frames: 30,
        opacity: 1.0,
        transform,
        animation,
        shadow: None,
        mask: None,
        content: ClipContent::Text(TextClip {
            text: id.to_string(),
            font_size: 20.0,
            color: ColorRgba(255, 255, 255, 255),
            align: Default::default(),
        }),
    }
}

fn expr_project(items: Vec<LayerItem>) -> Project {
    Project {
        canvas: Canvas {
            width: 400,
            height: 240,
            background: ColorRgba(0, 0, 0, 255),
        },
        timeline: Timeline {
            fps: Rational::new(30, 1).expect("fps"),
            total_frames: 30,
        },
        sources: vec![],
        layers: vec![Layer {
            id: "layer_a".to_string(),
            z_index: 0,
            items,
        }],
        audio: Default::default(),
    }
}

#[test]
fn resolves_canvas_refs_in_transforms() {
    let clip = text_clip(
        "clip_1",
        Transform {
            x: Scalar::Expr("canvas.width / 2".to_string()),
            y: Scalar::Expr("canvas.height / 2".to_string()),
            width: None,
            height: None,
            rotation_degrees: 0.0,
        },
        Default::default(),
    );
    let project = expr_project(vec![LayerItem::Clip(clip)]);
    let compiled = compile_project(&project).expect("compile");
    let op = compiled.operation(0).unwrap();
    let t = op.resolved_transform(0);
    assert_eq!(t.x, 200.0); // 400/2
    assert_eq!(t.y, 120.0); // 240/2
}

#[test]
fn resolves_forward_clip_references() {
    // clip_b references clip_a, which is defined first
    let clip_a = text_clip(
        "clip_a",
        Transform {
            x: Scalar::Literal(50.0),
            y: Scalar::Literal(0.0),
            width: Some(Scalar::Literal(100.0)),
            height: None,
            rotation_degrees: 0.0,
        },
        Default::default(),
    );

    let clip_b = text_clip(
        "clip_b",
        Transform {
            x: Scalar::Expr("clip_a.x + clip_a.width".to_string()),
            y: Scalar::Literal(0.0),
            width: None,
            height: None,
            rotation_degrees: 0.0,
        },
        Default::default(),
    );

    let project = expr_project(vec![LayerItem::Clip(clip_a), LayerItem::Clip(clip_b)]);
    let compiled = compile_project(&project).expect("compile");
    let ops = compiled.operation_indices_for_frame(0).unwrap();
    let op_b = compiled.operation(ops[1]).unwrap();
    assert_eq!(op_b.resolved_transform(0).x, 150.0);
}

#[test]
fn resolves_backward_clip_references() {
    // clip_a is defined first but references clip_b (backward reference)
    let clip_a = text_clip(
        "clip_a",
        Transform {
            x: Scalar::Expr("clip_b.x + 10".to_string()),
            y: Scalar::Literal(0.0),
            width: None,
            height: None,
            rotation_degrees: 0.0,
        },
        Default::default(),
    );

    let clip_b = text_clip(
        "clip_b",
        Transform {
            x: Scalar::Literal(50.0),
            y: Scalar::Literal(0.0),
            width: None,
            height: None,
            rotation_degrees: 0.0,
        },
        Default::default(),
    );

    let project = expr_project(vec![LayerItem::Clip(clip_a), LayerItem::Clip(clip_b)]);
    let compiled = compile_project(&project).expect("compile");
    let ops = compiled.operation_indices_for_frame(0).unwrap();
    let op_a = compiled.operation(ops[0]).unwrap();
    assert_eq!(op_a.resolved_transform(0).x, 60.0);
}

#[test]
fn rejects_circular_clip_references() {
    let clip_a = text_clip(
        "clip_a",
        Transform {
            x: Scalar::Expr("clip_b.x + 10".to_string()),
            y: Scalar::Literal(0.0),
            width: None,
            height: None,
            rotation_degrees: 0.0,
        },
        Default::default(),
    );

    let clip_b = text_clip(
        "clip_b",
        Transform {
            x: Scalar::Expr("clip_a.x + 10".to_string()),
            y: Scalar::Literal(0.0),
            width: None,
            height: None,
            rotation_degrees: 0.0,
        },
        Default::default(),
    );

    let project = expr_project(vec![LayerItem::Clip(clip_a), LayerItem::Clip(clip_b)]);
    let err = compile_project(&project).unwrap_err();
    assert!(matches!(err, CompileError::ExprError { .. }));
}

#[test]
fn rejects_self_referencing_expression() {
    let clip = text_clip(
        "clip_self",
        Transform {
            x: Scalar::Expr("clip_self.x + 10".to_string()),
            y: Scalar::Literal(0.0),
            width: None,
            height: None,
            rotation_degrees: 0.0,
        },
        Default::default(),
    );

    let project = expr_project(vec![LayerItem::Clip(clip)]);
    let err = compile_project(&project).unwrap_err();
    assert!(matches!(err, CompileError::ExprError { .. }));
}

#[test]
fn scales_expression_results() {
    let clip = text_clip(
        "clip_1",
        Transform {
            x: Scalar::Expr("canvas.width / 2".to_string()),
            y: Scalar::Literal(100.0),
            width: None,
            height: None,
            rotation_degrees: 0.0,
        },
        Default::default(),
    );
    let project = expr_project(vec![LayerItem::Clip(clip)]);
    let compiled = compile_project_with_scale(&project, 0.5).expect("compile");
    let op = compiled.operation(0).unwrap();
    let t = op.resolved_transform(0);
    // canvas.width at scale 0.5 = 200, then /2 = 100
    assert_eq!(t.x, 100.0);
    // literal 100 * 0.5 = 50
    assert_eq!(t.y, 50.0);
}

#[test]
fn rejects_unknown_ref_in_transform() {
    let clip = text_clip(
        "clip_1",
        Transform {
            x: Scalar::Expr("nonexistent.width".to_string()),
            y: Scalar::Literal(0.0),
            width: None,
            height: None,
            rotation_degrees: 0.0,
        },
        Default::default(),
    );
    let project = expr_project(vec![LayerItem::Clip(clip)]);
    let err = compile_project(&project).unwrap_err();
    assert!(matches!(err, CompileError::ExprError { .. }));
}

#[test]
fn rejects_parse_error_in_expression() {
    let clip = text_clip(
        "clip_1",
        Transform {
            x: Scalar::Expr("1 + + *".to_string()),
            y: Scalar::Literal(0.0),
            width: None,
            height: None,
            rotation_degrees: 0.0,
        },
        Default::default(),
    );
    let project = expr_project(vec![LayerItem::Clip(clip)]);
    let err = compile_project(&project).unwrap_err();
    assert!(matches!(err, CompileError::ExprError { .. }));
}
