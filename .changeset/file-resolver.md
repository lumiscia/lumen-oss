---
"@lumiscia/lumen-bindings": patch
---

**Rust API:** `ModelResolver`, `EnvironmentResolver` and `LutResolver` are replaced by one trait, `FileResolver`, whose `data` returns the file's bytes as a `Cow<'_, [u8]>`. `MediaResolver::Model`, `Environment` and `Lut` each hold a `Box<dyn FileResolver>`. Typed access to a file kind goes through the new markers `ModelFile`, `EnvironmentFile` and `LutFile`, as in `resolve_as::<ModelFile>(store, source)`, and `ResolverKind` names the resolver it returns as `ResolverKind::Resolver`. A resolver that keeps a file's bytes lends them without copying, which the browser preview now does instead of cloning each model, environment map or LUT file when the engine reads it; one that reads the file on each call returns the bytes it read, as before.
