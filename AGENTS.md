# Repository Guidelines

## Tooling

- Use `pnpm` for TypeScript workspace tasks.
- Run `pnpm install` after pulling remote changes and before starting work if dependencies may have changed.
- Use the root `oxfmt`/`oxlint` scripts while developing:
  - `pnpm format` to apply formatting
  - `pnpm format:check` to verify formatting
  - `pnpm lint` to run Oxlint
- Run `pnpm check`, `pnpm test`, and `pnpm build` before publishing substantial changes.
- Generated TypeScript definition files must be regenerated with `pnpm generate:types`; do not hand-edit generated outputs.
- WASM bindings are generated into `packages/lumen-bindings/src` with `just release`. This compiles the Rust WASM crate and can take a long time, especially on a clean checkout, so run it only when binding output or WASM-facing code needs to be validated.

## Definitions

- Lumen schema and node metadata live under `definitions/`.
- Treat `definitions/meta.json` as the source input for SDK type generation.
- Regenerate definitions with `just generate-definitions` after changing Rust node schemas, and use `just verify-definitions` to check that committed definitions are fresh.
- Keep generated SDK types in `packages/lumen-types/src/generated/`.
- If generated output is wrong, fix the generator or the upstream definitions rather than patching generated code by hand.

## Package Boundaries

- `packages/lumen-types` owns shared generated and hand-written public TypeScript types.
- `packages/lumen-shared` owns the dependency-free composition builder.
- Future runtime SDK packages should consume `@lumiscia/lumen-types` instead of duplicating schema-derived types.

## Code Style

- Prefer explicit exported types for public SDK surfaces.
- Keep generated code deterministic and stable across runs.
- Avoid introducing runtime dependencies into type-only packages unless they are truly needed.
- Keep package exports narrow and intentional.
- Do not rely on pre-commit hooks; CI is the source of truth for formatting, linting, tests, and builds.
- If `lumen-bindings` imports fail during local TypeScript checks, generate the bindings with `just release` rather than adding handwritten stubs.

## Git

- Do not revert unrelated changes.
- Commit messages should follow the style used in the Lumen repos, for example:
  - `chore: update generated types`
  - `feat(types): add composition types`
  - `fix(node): update schema references`
