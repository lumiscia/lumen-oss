# Lumen Definitions

Generated JSON schemas and metadata for Lumen compositions.

This repository contains machine-readable artifacts generated from the canonical Lumen source:

- `meta.json` - node kinds, node specs, ports, properties, defaults, and schema metadata
- `schemas/meta.schema.json` - JSON Schema for `meta.json`
- `schemas/composition.schema.json` - JSON Schema for Lumen composition documents

These files are generated automatically and should not be edited by hand.

## Usage

Reference the schemas directly from this repository, or vendor them into tooling that needs to validate Lumen compositions.

```json
{
  "$schema": "https://raw.githubusercontent.com/lumiscia/lumen-definitions/main/schemas/composition.schema.json"
}
```

## Updates

Changes are published from the upstream Lumen source repository. Each update PR includes the source commit used to generate the artifacts so schema changes can be traced back to the exact Lumen revision.
