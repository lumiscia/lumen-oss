---
"@lumiscia/lumen-types": minor
---

The types of what the engine returns while it runs (diagnostics, the media inventory, media failures, model info, the audio plan and frame requirements) are generated from the engine's own Rust types, so they can no longer drift from the JSON it writes. Their schema is `definitions/runtime.schema.json`.

Some exports have new names:

| Before                    | Now                             |
| ------------------------- | ------------------------------- |
| `LumenDiagnostic`         | `Diagnostic`                    |
| `LumenDiagnosticSeverity` | `Severity`                      |
| `LumenModelInfo`          | `ModelInfo`                     |
| `LumenModelAnimation`     | `AnimationInfo`                 |
| `LumenModelMaterial`      | `MaterialInfo`                  |
| `LumenModelCamera`        | `CameraInfo`                    |
| `NodeParamLocation`       | `NodeParamAt`                   |
| `MediaFailureKind`        | `AssetKind` (also names `font`) |

`Diagnostic.code` is now a `DiagnosticCode`, a union of every code the engine reports that still accepts any other string, because a newer engine can add codes. `diagnosticCodes` lists each code with its severity:

```ts
import { diagnosticCodes, type Diagnostic } from "@lumiscia/lumen-types";

const warnings = new Set<string>(
  diagnosticCodes.filter(({ severity }) => severity === "warning").map(({ code }) => code),
);
const isWarning = (diagnostic: Diagnostic) => warnings.has(diagnostic.code);
```

The kinds in a `MediaInventory` are `AssetKind`s, which include `font`, where they were `MediaSourceKind`s. `MediaSourceKind` itself is unchanged.
