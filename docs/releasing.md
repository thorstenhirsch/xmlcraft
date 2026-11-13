# Releasing XmlCraft

> **Last reviewed:** 2026-11-04 · **Last updated:** 2026-11-04 · **Change:** 0.1.0 is released (v0.1.0, unsigned on Windows, ad-hoc signed on macOS), the credits are refreshed before a release, the pipeline is built and verified (M0.10, M0.11)

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

1. Bump the version with `cargo xtask version set X.Y.Z` and refresh the credits of the About window with `cargo xtask contributors --write` (needs `gh` to tell a commit's author by their GitHub username; `--no-github` uses only `noreply` addresses) in a PR; merge when CI is green.
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

## The macOS disk image

The Finder window's artwork and icon positions come from committed files in `packaging/macos/dmg/`: `package.sh` copies the rendered background (`background.tiff`) and the Finder layout (`dmg-layout.DS_Store`) into the image and `verify.sh` checks them there; `background.svg` is the source artwork and is not shipped. How to change them, and why the volume name has no version: `packaging/macos/dmg/README.md`. The two Python helpers there are a packaging-only exception to "Rust only" (approved by the owner, 2026-11-04) and never run in the build.

## Known gaps

- The Windows installer has no setup dialogs (welcome, licence, folder): it installs per machine with a progress bar. `INSTALLDESKTOPSHORTCUT=0` skips the desktop shortcut.
- No Flatpak, FreeBSD or web builds (not planned for 0.1.0).

## Revision history

- 2026-10-27: created.
- 2026-10-27: workflows and scripts written (M0.11).
- 2026-10-27: first dry run (40425116751): macOS and Linux built; both Windows jobs failed at `wix build` (WIX0367). Second dry run (40454851007), after #4: all six targets built and verified. The Windows ARM64 workflow installed and ran the arm64 MSI; the other artifacts are built and verified but not yet installed on their platforms.
- 2026-10-29: the credits of the About window (`cargo xtask contributors --write`) are part of the version bump.
- 2026-11-04: 0.1.0 released from `main` at f5f40ed (run 41894801791): 18 files and `SHA256SUMS.txt` in the draft, all checksums verified, published as v0.1.0. No signing secrets were set, so the Windows builds are unsigned and the macOS app is only ad-hoc signed (not notarised).
- 2026-11-05: the macOS disk image has a window of its own (background, icon positions), and the app has a new logo (a fox between angle brackets).
- 2026-11-07: 0.2.1 released from `main` at 9f08722 (run 42323651053): 19 files and `SHA256SUMS.txt` in the release, published as v0.2.1. It carries generic clean-room wording, the tab menu and the log file. No signing secrets were set, so the Windows builds are unsigned and the macOS app is only ad-hoc signed.
- 2026-11-06: 0.2.0 released from `main` at d29aa74 (run 42218123193): 18 files and `SHA256SUMS.txt`, all checksums verified, published as v0.2.0; unsigned on Windows, ad-hoc signed on macOS.
- 2026-11-13: 0.3.0 released from `main` at 31cdc25 (run 43428488168): 18 files and `SHA256SUMS.txt` in the draft, all 18 checksums verified, the macOS command-line tool smoke-run (`xmlcraft-cli 0.3.0`, 82 tools), published as v0.3.0. Still unsigned on Windows and ad-hoc signed on macOS. The release was held up by a dependency update on `main` (a `ureq` bump that stopped refusing over-limit downloads of undeclared length, a `skrifa` lint, a `getrandom` wasm feature): fixed before the release (#164). The apps were not launched on every platform; the desktop builds were verified by the workflow's own checks.
