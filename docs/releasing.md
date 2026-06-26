# Cutting a release

Maintainer runbook. Contributors do not need this; see [`CONTRIBUTING.md`](../CONTRIBUTING.md).

Crates publish in dependency order, since `cargo publish` resolves path dependencies against the real index: `winter-proto`, then `winter-render`, `winter-core`, `winter-client`, and `winter-term` last. Recent Cargo resolves that order itself for a workspace publish.

1. Bump the version in the root `Cargo.toml`, in both `[workspace.package].version` and each internal entry under `[workspace.dependencies]`, then run `cargo check` so `Cargo.lock` follows.
2. Move the `Unreleased` items in [`CHANGELOG.md`](../CHANGELOG.md) under a new `## [x.y.z]` heading. The release workflow uses that section verbatim as the release body, so write it for users.
3. Commit, then `git tag vx.y.z && git push origin vx.y.z`.
4. [`.github/workflows/release.yml`](../.github/workflows/release.yml) builds the `.deb`, `.dmg`, and `.exe` and opens a **draft** release with them attached. Review and publish it.
5. Update the downstream manifests against the published assets: `packaging/aur/PKGBUILD` (see its header), `packaging/scoop/winter-term.json` (version plus the installer's SHA256), and winget (see `packaging/winget/README.md`).

`workflow_dispatch` runs the same build without cutting a release, for rehearsing a packaging change.
