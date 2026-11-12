---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-types": minor
"@lumiscia/lumen-shared": minor
"@lumiscia/lumen-sdk": minor
---

**Breaking:** `lumenSchemaVersion` is now the Lumen `major.minor` a composition was written for, `"0.6"` for the 0.6 line, instead of the frozen `"0.1.0"`. An engine reads only its own line: it accepts its `major.minor` or a full `major.minor.patch` of that line (`"0.6.3"`), and rejects everything else, including today's `"0.1.0"`, with `document.schema_version`:

```
this composition declares schema version 0.1.0; this engine reads 0.6, so it was written for a different Lumen version
```

Every stored composition that declares `"lumenSchemaVersion": "0.1.0"` must change it to `"0.6"`. A composition that leaves the field out is still read as the engine's own version. 0.x minor releases are breaking, so there is no migration between lines.

`Composition.toJSON()` in the SDK always writes the current version, so compositions built with the SDK need no change, and the `lumenSchemaVersion` option of `Composition` is removed. `@lumiscia/lumen-types` exports `lumenSchemaVersion` (`"0.6"`), `lumen_engine::SCHEMA_VERSION` is the Rust side, and the JSON schema constrains the property to `^0\.6(\.[0-9]+)?$`. `just set-schema-version <major.minor>` moves the engine constant, the tracked fixtures and docs, and the generated output together.
