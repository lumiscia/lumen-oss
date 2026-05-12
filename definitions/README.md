# Lumen Definitions

Generated JSON schema and metadata annotations for Lumen compositions.

This repository contains machine-readable artifacts generated from the canonical Lumen source:

- `composition.schema.json` - JSON Schema for Lumen composition documents, including `x-lumen-*` annotations for node metadata, property metadata, defaults, ports, and enum labels

These files are generated automatically and should not be edited by hand.

## Usage

Reference the schemas directly from this repository, or vendor them into tooling that needs to validate Lumen compositions.

```json
{
  "$schema": "https://raw.githubusercontent.com/lumiscia/lumen/main/definitions/composition.schema.json"
}
```

## Updates

Regenerate these files from the repository root with:

```bash
just generate-definitions
```

CI runs `just verify-definitions` to compare the committed files against freshly generated output. If a node schema changes, update the Rust source first, regenerate `definitions/`, and commit both changes together.
