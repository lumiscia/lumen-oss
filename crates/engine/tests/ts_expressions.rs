//! Parses and evaluates the expressions `@lumiscia/lumen-shared` emits. The fixture is generated
//! by `packages/lumen-shared/test/expression.test.ts`; regenerate it with
//! `just generate-expression-fixture`.

#![cfg(feature = "json")]

use std::collections::BTreeMap;

use lumen_engine::{
    expr::{BUILTINS, Expression, ExpressionContext, ExpressionValue, FunctionScope},
    json,
};
use serde_json::Value;

/// Font metrics vary by machine, so these cases have no expected value; they must satisfy the
/// finite and positive.
const MEASUREMENTS: [&str; 2] = ["text_height", "text_width"];

const FIXTURE: &str = include_str!("fixtures/ts_expressions.json");

#[test]
fn typescript_helper_expressions_round_trip() {
    let fixture: Value = serde_json::from_str(FIXTURE).unwrap();
    let composition = json::parse_value(&fixture["composition"]).unwrap();
    let functions: BTreeMap<String, String> =
        serde_json::from_value(fixture["composition"]["functions"].clone()).unwrap();
    let _scope = FunctionScope::enter(&functions).unwrap();
    let props = BTreeMap::from([("title".to_string(), ExpressionValue::String("Lumen".into()))]);
    let ctx = ExpressionContext {
        frame: fixture["frame"].as_f64().unwrap(),
        fps: composition.timeline().fps,
        width: composition.render_settings().width,
        height: composition.render_settings().height,
        graph: Some(composition.graph()),
        props: Some(&props),
        ..Default::default()
    };

    let cases = fixture["cases"].as_object().unwrap();
    for spec in BUILTINS {
        assert!(cases.contains_key(spec.name), "no case for `{}`", spec.name);
    }
    let mut values = BTreeMap::new();
    for (name, case) in cases {
        let source = case["expression"].as_str().unwrap();
        let body = source.strip_prefix('=').unwrap();
        let value = Expression::parse(body)
            .unwrap_or_else(|error| panic!("{name}: `{source}` does not parse: {error}"))
            .evaluate(&ctx)
            .unwrap_or_else(|error| panic!("{name}: `{source}` does not evaluate: {error}"));
        match case.get("expected") {
            Some(expected) => assert!(
                matches(&value, expected),
                "{name}: `{source}` gave {value:?}, expected {expected}"
            ),
            None => assert!(
                MEASUREMENTS.contains(&name.as_str()),
                "{name}: `{source}` has neither an expected value nor an invariant"
            ),
        }
        values.insert(name.as_str(), value);
    }

    let measure = |source: &str| match Expression::parse(source).unwrap().evaluate(&ctx).unwrap() {
        ExpressionValue::Number(number) => number,
        other => panic!("`{source}` gave {other:?}, expected a number"),
    };
    for name in MEASUREMENTS {
        let ExpressionValue::Number(number) = values[name] else {
            panic!("{name} gave {:?}, expected a number", values[name]);
        };
        assert!(number.is_finite() && number > 0.0, "{name} gave {number}");
    }
    let width = measure("text_width('Hello', 32, 300, 'Roboto')");
    let height = measure("text_height('Hello', 32, 300, 'Roboto')");
    assert!(
        height > 0.0 && width > height,
        "'Hello' should be wider than tall: {width} x {height}"
    );
}

fn matches(value: &ExpressionValue, expected: &Value) -> bool {
    let close = |actual: f64, expected: &Value| {
        expected
            .as_f64()
            .is_some_and(|expected| (actual - expected).abs() <= 1e-9 * expected.abs().max(1.0))
    };
    match (value, expected) {
        (ExpressionValue::Number(number), expected) => close(*number, expected),
        (ExpressionValue::Vec2((x, y)), Value::Array(expected)) => {
            expected.len() == 2 && close(*x, &expected[0]) && close(*y, &expected[1])
        }
        (ExpressionValue::Color(color), expected @ Value::String(_)) => {
            json::parse_color(expected)
                == Some(color.map(|channel| (channel * 255.0).round() as u8))
        }
        (ExpressionValue::String(text), Value::String(expected)) => text == expected,
        _ => false,
    }
}
