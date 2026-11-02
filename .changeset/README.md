# Changesets

This directory stores release notes for package changes.

Run `pnpm changeset` when a change should be released. The manual Version Packages workflow runs `pnpm release:version` to apply pending changesets, sync the Rust workspace version to the TypeScript package version, refresh lockfiles, and open a version PR. After that PR merges, the manual Release workflow builds generated bindings, runs `cargo publish --workspace`, publishes npm packages, tags the release, and creates a summarized GitHub Release.
