---
"@lumiscia/lumen-bindings": minor
---

`node(id, 'param')`, default params in the schema and nested `to_json` now convert a param directly rather than through `PropertyValue`, and reading a param that holds an expression no longer clones that expression. Compositions and the generated schema are unchanged. For Rust: `get_property` returns `PropertyRead`, a borrow of the expression or the literal's value, `NodeSchemaDef::default_properties` holds JSON, and `ParamValue` gains `to_json_value` and `to_expression`.
