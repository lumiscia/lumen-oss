# Changesets

This directory stores release notes for package changes.

Run `pnpm changeset` when a change should be released. The manual Version Packages workflow runs `pnpm release:version` to apply pending changesets, sync the Rust workspace version to the TypeScript package version, refresh lockfiles, and open a version PR. After that PR merges, the manual Publish Packages workflow runs `pnpm release:publish`, which builds generated bindings, publishes crates in dependency order, and then publishes npm packages.
