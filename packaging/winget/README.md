# winget manifest

winget manifests live in [microsoft/winget-pkgs](https://github.com/microsoft/winget-pkgs), not here. This directory holds the source of truth that gets submitted there, so the values are reviewed in this repo rather than reconstructed at submission time.

## Submitting a release

The maintained path is [`wingetcreate`](https://github.com/microsoft/winget-create), which builds the three-file manifest, computes the installer hash, validates it, and opens the pull request:

```powershell
wingetcreate update QuangTrungTa.WinterTerminal `
    --version 0.1.0 `
    --urls https://github.com/taquangtrung/winter-term/releases/download/v0.1.0/winter-terminal-0.1.0-setup.exe `
    --submit
```

For the very first submission, use `wingetcreate new` with that URL instead and fill in the metadata from `manifest-values.yaml` in this directory.

## Why not commit the full manifest here

winget requires the installer's SHA256, which only exists once the release asset is published. A manifest committed ahead of the release would carry a placeholder hash and fail validation, so only the stable metadata is kept.
