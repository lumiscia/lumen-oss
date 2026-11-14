---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-preview": minor
---

The preview controller takes models, environment maps and LUTs through one set of methods that name the kind, instead of three of each. `setFile`, `removeFile` and `hasFile` replace `setModel`, `setEnvironment`, `setLut`, `removeModelSource`, `removeEnvironmentSource`, `removeLutSource`, `hasModel`, `hasEnvironment` and `hasLut`. The kind is `"model"`, `"environment"` or `"lut"`, and any other kind throws.

```ts
// Before
controller.setModel("fox", bytes);
controller.removeModelSource("fox");

// After
controller.setFile("model", "fox", bytes);
controller.removeFile("model", "fox");
controller.hasFile("model", "fox"); // true or false
```

In `@lumiscia/lumen-preview/advanced`, `LumenMediaTarget` has the same three methods, and all of its members are required. `LumenMediaBridge` registers these files with `registerFile(kind, id, source)` in place of `registerModelSource`, `registerEnvironmentSource` and `registerLutSource`, and `registerVideoSource` takes its options as an object, no longer a bare fps number. `registerVideoSourceBytes` is removed; pass the bytes to `registerVideoSource`.
