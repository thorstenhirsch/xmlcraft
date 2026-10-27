#!/usr/bin/env bash
# Check the macOS artifacts package.sh built, as a user would meet them: the DMG mounts, the app inside is a signed bundle with the
# right identity and version, both binaries hold the architectures the artifact name promises, and the CLI runs.
#
# Usage: packaging/macos/verify.sh [--arch universal|aarch64|x86_64]
set -euo pipefail
# shellcheck source=../env.sh
. "$(dirname "${BASH_SOURCE[0]}")/../env.sh"

ARCH=universal
while [[ $# -gt 0 ]]; do
  case "$1" in
    --arch) ARCH="$2"; shift 2 ;;
    -h | --help) sed -n '2,6p' "$0"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done
case "$ARCH" in
  universal) WANT="arm64 x86_64" ;;
  aarch64) WANT="arm64" ;;
  x86_64) WANT="x86_64" ;;
  *) echo "unknown --arch $ARCH" >&2; exit 2 ;;
esac

DMG="$DIST/xmlcraft-$VERSION-macos-$ARCH.dmg"
ZIP="$DIST/xmlcraft-cli-$VERSION-macos-$ARCH.zip"
[[ -f "$DMG" ]] || { echo "error: $DMG not found" >&2; exit 1; }
[[ -f "$ZIP" ]] || { echo "error: $ZIP not found" >&2; exit 1; }

WORK="$(mktemp -d)"
MOUNT="$WORK/mount"
mkdir -p "$MOUNT"
cleanup() {
  hdiutil detach "$MOUNT" >/dev/null 2>&1 || true
  rm -rf "$WORK"
}
trap cleanup EXIT

fail() { echo "error: $*" >&2; exit 1; }

check_archs() {
  local file="$1" have want
  have="$(lipo -archs "$file")"
  for want in $WANT; do
    case " $have " in *" $want "*) ;; *) fail "$file has architectures [$have], expected $WANT" ;; esac
  done
  [[ "$(wc -w <<<"$have")" = "$(wc -w <<<"$WANT")" ]] || fail "$file has architectures [$have], expected exactly $WANT"
  echo "ok $(basename "$file"): $have"
}

hdiutil attach -nobrowse -readonly -mountpoint "$MOUNT" "$DMG" >/dev/null
APP="$MOUNT/XmlCraft.app"
[[ -d "$APP" ]] || fail "XmlCraft.app is not in the disk image"
[[ -L "$MOUNT/Applications" ]] || fail "the disk image has no Applications shortcut"
codesign --verify --strict --deep --verbose=1 "$APP"
plutil -lint "$APP/Contents/Info.plist" >/dev/null
plist() { plutil -extract "$1" raw -o - "$APP/Contents/Info.plist"; }
[[ "$(plist CFBundleIdentifier)" = "io.github.thorstenhirsch.xmlcraft" ]] || fail "wrong bundle identifier: $(plist CFBundleIdentifier)"
[[ "$(plist XmlCraftVersion)" = "$VERSION" ]] || fail "Info.plist version $(plist XmlCraftVersion) is not $VERSION"
[[ "$(plist CFBundleExecutable)" = "XmlCraft" ]] || fail "wrong executable name"
[[ -f "$APP/Contents/Resources/$(plist CFBundleIconFile).icns" ]] || fail "the bundle icon is missing"
check_archs "$APP/Contents/MacOS/XmlCraft"

unzip -q "$ZIP" -d "$WORK/cli"
CLI="$(find "$WORK/cli" -name xmlcraft-cli -type f | head -n 1)"
[[ -n "$CLI" ]] || fail "xmlcraft-cli is not in the zip"
check_archs "$CLI"
codesign --verify --strict --verbose=1 "$CLI"
# The CLI runs on this machine only when it holds this machine's architecture.
if lipo -archs "$CLI" | grep -qw "$(uname -m | sed 's/arm64/arm64/')"; then
  out="$("$CLI" --version)"
  [[ "$out" = "xmlcraft-cli $VERSION" ]] || fail "xmlcraft-cli --version said '$out'"
  echo "ok xmlcraft-cli --version: $out"
fi
echo "macOS artifacts verified for $ARCH"
