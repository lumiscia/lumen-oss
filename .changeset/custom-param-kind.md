---
"@lumiscia/lumen-bindings": minor
---

In the Rust API, `PropertyKind` keeps one variant per scalar kind (`Float`, `Int`, `Bool`, `String`, `Color`, `Vec2`, `Vec3`, `Enum`). The kinds of compound params, such as text content, paint and region, become `PropertyKind::Custom(&ParamTypeDef)`. A `ParamTypeDef` carries the kind's `name`, which is the published `x-lumen-kind`, and with the `metadata` feature a `schema` function for the JSON schema of a value. The composition schema and the generated types are unchanged.

```rust
// before
match property.expected {
    PropertyKind::TextContent => /* ... */,
    _ => /* ... */,
}

// after
match property.expected {
    PropertyKind::Custom(def) if def.name == "text_content" => /* ... */,
    _ => /* ... */,
}
```

A `Delegate` enum with no `#[delegate(kind = ...)]` implements `NodeParamType` by hand and returns its own `PropertyKind::Custom`.
