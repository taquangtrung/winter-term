# Cutting a release

Maintainer runbook. Contributors do not need this; see [`CONTRIBUTING.md`](CONTRIBUTING.md).

Crates publish in dependency order, since `cargo publish` resolves path dependencies against the real index, and the layers under `crates/` give that order: first the crates that depend on no other of ours (`winter-proto`, `winter-base`, `winter-lsp`, and `winter-ext-api`), then the rest of the `sdk` and `base` crates, then the `libs` crates, then the `tools`, then `winter-term` last; `winter-client` needs only `winter-proto`. Recent Cargo resolves that order itself for a workspace publish. `winter-simple-ext` is published because `winter-term` names it as an optional dependency (the `example-extension` feature), which crates.io requires to be on the index.

1. Bump the version in the root `Cargo.toml`, in both `[workspace.package].version` and each internal entry under `[workspace.dependencies]`, then run `cargo check` so `Cargo.lock` follows.
2. Move the `Unreleased` items in [`CHANGELOG.md`](CHANGELOG.md) under a new `## [x.y.z]` heading. The release workflow uses that section verbatim as the release body, so write it for users.
3. Commit, then `git tag vx.y.z && git push origin vx.y.z`.
4. [`.github/workflows/release.yml`](.github/workflows/release.yml) builds the `.deb`, `.dmg`, and `.exe` and opens a **draft** release with them attached. Review and publish it.
5. Update the downstream manifests against the published assets: `packaging/linux/aur/PKGBUILD` (see its header), `packaging/windows/scoop/winter-term.json` (version plus the installer's SHA256), and winget (see `packaging/windows/winget/README.md`).

`workflow_dispatch` runs the same build without cutting a release, for rehearsing a packaging change.
