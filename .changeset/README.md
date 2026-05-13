# Changesets

This directory stores release notes for package changes.

Run `pnpm changeset` when a change should be released. CI runs `pnpm release:version` to apply pending changesets, sync the Rust workspace version to the TypeScript package version, and refresh lockfiles. Publishing runs `pnpm release:publish`, which builds generated bindings, publishes crates in dependency order, and then publishes npm packages.
