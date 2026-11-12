# Changesets

This directory stores release notes for package changes.

Run `pnpm changeset` when a change should be released. The manual Version Packages workflow runs `pnpm release:version` to apply pending changesets, sync the Rust workspace version to the TypeScript package version, refresh lockfiles, and open a version PR. After that PR merges, the manual Release workflow builds generated bindings, runs `cargo publish --workspace`, publishes npm packages, tags the release, and creates a summarized GitHub Release.

A new release line also gets its schema version from the Version Packages workflow, which runs `just set-schema-version` when the new `major.minor` differs from the engine's.

## Prereleases

`pnpm changeset pre enter beta` starts a prerelease line: commit the `.changeset/pre.json` it writes, and from then on the Version Packages workflow produces `x.y.z-beta.0`, `-beta.1` and so on. The Release workflow publishes those to npm under the `beta` tag instead of `latest`, to crates.io as prerelease versions, and as a GitHub prerelease. Applied changesets move to `.changeset/pre/` and stay there until the line ends.

`pnpm changeset pre exit`, committed, ends the line: the next Version Packages run produces `x.y.z` with every change since the last stable release in its changelog.

Compositions declare the same `lumenSchemaVersion` in every prerelease of a line as in its release, so a schema change between two prereleases is not detected from the document.
