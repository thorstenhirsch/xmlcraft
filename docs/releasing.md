# Releasing XmlCraft

> **Last reviewed:** 2026-10-27 · **Last updated:** 2026-10-27 · **Change:** initial; the pipeline is planned, nothing is built yet (arrives with M0.10 and M0.11)

## Targets

| Platform | Runner | Target | Artifacts |
|---|---|---|---|
| macOS universal (Intel + Apple Silicon) | `macos-15` | `aarch64-apple-darwin` + `x86_64-apple-darwin` joined with `lipo` | `.dmg`, CLI `.zip` |
| Windows x64 | `windows-latest` | `x86_64-pc-windows-msvc` | `.msi`, portable `.zip` |
| Windows arm64 | `windows-latest` | `aarch64-pc-windows-msvc`, cross-compiled on x64 | `.msi`, portable `.zip` |
| Linux x86_64 | `ubuntu-22.04` | native | `.deb`, `.rpm`, `.tar.gz` |
| Linux aarch64 | `ubuntu-22.04-arm` | native | `.deb`, `.rpm`, `.tar.gz` |

Artifact names follow `xmlcraft-<version>-<platform>-<arch>.<ext>`. The version lives only in `[workspace.package]` in `Cargo.toml` and is changed with `cargo xtask version set X.Y.Z`.

## Process

1. Bump the version with `cargo xtask version set X.Y.Z` in a PR; merge when CI is green.
2. Push `main` to the `release` branch (or run the Release workflow by hand with an optional version for a dry run).
3. The workflow builds every target and creates or updates a **draft** GitHub Release with `SHA256SUMS.txt`. Publishing the draft creates the tag; `-rc` versions are marked pre-release.

## Signing

Signing runs only when its secrets exist; otherwise the artifact ships unsigned with a workflow warning. Configure them in a `release` environment restricted to the `release` branch:

- macOS: `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`, `KEYCHAIN_PASSWORD`, `APPLE_ID`, `APPLE_PASSWORD`, `APPLE_TEAM_ID` (hardened runtime, notarisation and stapling). Without them the app is ad-hoc signed.
- Windows: `WINDOWS_CERTIFICATE` and `WINDOWS_CERTIFICATE_PASSWORD`, or Azure Trusted Signing (`AZURE_*`).

Unsigned builds trigger Gatekeeper and SmartScreen warnings; the README documents the workaround.

## One-time values that never change

- The Windows MSI `UpgradeCode` GUID (recorded here when it is generated in M0.11).
- The app id `io.github.thorstenhirsch.xmlcraft` (macOS bundle id, Linux desktop and metainfo names).

## Revision history

- 2026-10-27: created.
