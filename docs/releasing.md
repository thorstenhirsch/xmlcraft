# Releasing XmlCraft

> **Last reviewed:** 2026-10-27 · **Last updated:** 2026-10-27 · **Change:** initial; the pipeline is planned, nothing is built yet (arrives with M0.10 and M0.11)

## Targets

| Platform | Runner | Target | Artifacts |
|---|---|---|---|
| macOS universal (Intel + Apple Silicon) | `macos-15` | `aarch64-apple-darwin` + `x86_64-apple-darwin` joined with `lipo` | `xmlcraft-<v>-macos-universal.dmg`, `xmlcraft-cli-<v>-macos-universal.zip` |
| Windows x64 | `windows-latest` | `x86_64-pc-windows-msvc` | `xmlcraft-<v>-windows-x64.msi`, `…-portable.zip` |
| Windows arm64 | `windows-latest` (cross-compiled) | `aarch64-pc-windows-msvc` | `xmlcraft-<v>-windows-arm64.msi`, `…-portable.zip` |
| Linux x86_64 | `ubuntu-22.04` (glibc 2.35) | native | `.AppImage` (+ `.zsync`), `.deb`, `.rpm`, `.tar.gz`, and `xmlcraft-cli-<v>-linux-x86_64.tar.gz` |
| Linux aarch64 | `ubuntu-22.04-arm` | native | the same set |

Artifact names follow `xmlcraft-<version>-<platform>-<arch>.<ext>`; `SHA256SUMS.txt` accompanies the release. The version lives only in `[workspace.package]` in `Cargo.toml` and is changed with `cargo xtask version set X.Y.Z`. The scripts are in `packaging/` (`macos/package.sh`, `windows/package.ps1`, `linux/package.sh`); each can be run locally on its own platform.

## Process

1. Bump the version with `cargo xtask version set X.Y.Z` in a PR; merge when CI is green.
2. **Dry run (publishes nothing):** Actions ▸ Release ▸ Run workflow, optionally with a version such as `0.1.0-rc.1`. Every artifact is built, verified and attached to the run for 14 days.
3. **Release:** push `main` to the `release` branch. The workflow builds every target and creates or updates a **draft** GitHub Release with `SHA256SUMS.txt`. Publishing the draft creates the tag; `-rc` versions are marked pre-release. A release that is already published is never touched: bump the version first.
4. The weekly **Windows ARM64** workflow installs the arm64 MSI on a real Windows 11 ARM64 machine and runs the command line, because an arm64 build made on an x64 runner cannot run there.

## Signing

Signing runs only when its secrets exist; otherwise the artifact ships unsigned with a workflow warning. Set them in a `release` environment restricted to the `release` branch:

- macOS: `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`, `KEYCHAIN_PASSWORD`, `APPLE_ID`, `APPLE_PASSWORD`, `APPLE_TEAM_ID` (hardened runtime, notarisation and stapling). Without them the app is ad-hoc signed, and macOS asks the user to allow it in System Settings ▸ Privacy & Security.
- Windows: `WINDOWS_CERTIFICATE` and `WINDOWS_CERTIFICATE_PASSWORD`, or Azure Trusted Signing (`AZURE_TENANT_ID`, `AZURE_CLIENT_ID`, `AZURE_CLIENT_SECRET`, `AZURE_SIGNING_ENDPOINT`, `AZURE_SIGNING_ACCOUNT`, `AZURE_CERT_PROFILE`). Without them Windows SmartScreen warns on first run ("More info" ▸ "Run anyway").

## One-time values that never change

- The Windows MSI `UpgradeCode`: `{5CE729A2-80AA-4E57-8DC3-6B3014812640}` (in `packaging/windows/xmlcraft.wxs`, checked by `test-msi.ps1`). Changing it makes an update install beside the old version instead of replacing it.
- The app id `io.github.thorstenhirsch.xmlcraft` (macOS bundle id, Linux desktop and metainfo names).

## Known gaps

- The Windows installer has no setup dialogs (welcome, licence, folder): it installs per machine with a progress bar. `INSTALLDESKTOPSHORTCUT=0` skips the desktop shortcut.
- The macOS disk image has no background artwork.
- No Flatpak, FreeBSD or web builds (not planned for 0.1.0).

## Revision history

- 2026-10-27: created.
- 2026-10-27: workflows and scripts written (M0.11); first dry run pending.
