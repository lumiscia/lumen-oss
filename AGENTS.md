# Repository Guidelines

## Tooling

- Use `pnpm` through the root scripts for TypeScript workspace tasks.
- Run `pnpm install` after pulling remote changes and before starting work if dependencies may have changed.
- Run `pnpm check` and `pnpm build` to validate changes.
- Generated TypeScript definition files must be regenerated with `pnpm generate:types`; do not hand-edit generated outputs.

## Definitions

- Lumen schema and node metadata live under `vendor/lumen-definitions`.
- Treat `vendor/lumen-definitions` as the source input for SDK type generation.
- Keep generated SDK types in `packages/lumen-types/src/generated/`.
- If generated output is wrong, fix the generator or the upstream definitions rather than patching generated code by hand.

## Package Boundaries

- `packages/lumen-types` owns shared generated and hand-written public TypeScript types.
- Future runtime SDK packages should consume `@lumen-sdk/lumen-types` instead of duplicating schema-derived types.

## Code Style

- Prefer explicit exported types for public SDK surfaces.
- Keep generated code deterministic and stable across runs.
- Avoid introducing runtime dependencies into type-only packages unless they are truly needed.
- Keep package exports narrow and intentional.

## Git

- Do not revert unrelated changes.
- Commit messages should follow the style used in the Lumen repos, for example:
  - `chore: update generated types`
  - `feat(types): add composition types`
  - `fix(node): update schema references`
