---
"@lumiscia/lumen-bindings": minor
---

Params now parse JSON through one path, with no intermediate value type. Every composition that parsed before parses to the same params, and the generated schema is unchanged. Error text changes in two ways: nested params name one full path, such as ``node 1: `content[0].underline.thickness` expected float`` in place of ``node 1: `content`: [0]: `underline`: `thickness` expected float``, and an expression where a param takes literals, or an integer out of range, now says so directly, such as ``node 7: `width` must be an integer from 0 to 4294967295, got -1``.

For Rust: `PropertyValue`, `PropertyExpression`, `DelegateValue`, `OptionalDelegate`, `DelegateVec`, `DeferredJsonValue` and `#[delegate(properties)]` are gone, and delegates no longer implement `Deserialize`. `DelegateEvaluable` gains `from_json_value`, `to_json_value` and `read`, and `ParamValue` gains `from_json_value`.
