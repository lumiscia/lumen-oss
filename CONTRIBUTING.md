# Contributing

Thanks for helping improve Lumen.

## Development

Install dependencies with Vite+:

```bash
vp install
```

Before opening a pull request, run:

```bash
vp check
vp test
cargo fmt
cargo check
cargo test
```

Generated definitions and migrations should not be edited by hand. Use the generator commands in the `justfile`.

## Scope

This repository contains the open renderer, schema, TypeScript package, and self-hostable runtime pieces. Hosted platform features such as billing, team auth, dashboards, and managed storage are intentionally outside this repository.
