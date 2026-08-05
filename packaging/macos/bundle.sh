#!/usr/bin/env bash
#
# Assemble a macOS .app bundle around the compiled mavlog binary.
#
# A bare Unix executable double-clicked in Finder is handed to Terminal.app,
# which is why running the raw binary pops a terminal window alongside the GUI.
# Wrapping it in a .app bundle makes LaunchServices treat it as a proper GUI
# app: no terminal, correct Dock icon, correct menu-bar name.
#
# Usage:
#   packaging/macos/bundle.sh <binary> <output-dir> [version]
#
#   <binary>      path to the built `mavlog` executable
#   <output-dir>  directory to create `mavlog.app` in (created if missing)
#   [version]     version string for Info.plist; defaults to the version in
#                 Cargo.toml. A leading "v" (e.g. from a git tag) is stripped.
set -euo pipefail

BIN="${1:?usage: bundle.sh <binary> <output-dir> [version]}"
OUT_DIR="${2:?usage: bundle.sh <binary> <output-dir> [version]}"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

# Resolve the version: explicit arg wins, otherwise read it from Cargo.toml.
VERSION="${3:-$(grep -m1 '^version' "$REPO_ROOT/Cargo.toml" | sed -E 's/.*"([^"]+)".*/\1/')}"
VERSION="${VERSION#v}" # strip a leading "v" so git tags like v0.1.0 work

if [[ ! -f "$BIN" ]]; then
	echo "error: binary not found: $BIN" >&2
	exit 1
fi

APP="$OUT_DIR/mavlog.app"
CONTENTS="$APP/Contents"

echo "Bundling $BIN -> $APP (version $VERSION)"

rm -rf "$APP"
mkdir -p "$CONTENTS/MacOS" "$CONTENTS/Resources"

# The binary's filename inside MacOS/ must match CFBundleExecutable.
cp "$BIN" "$CONTENTS/MacOS/mavlog"
chmod +x "$CONTENTS/MacOS/mavlog"

# Substitute the version into the Info.plist template.
sed "s/__VERSION__/$VERSION/g" "$SCRIPT_DIR/Info.plist" > "$CONTENTS/Info.plist"

# Icon is optional: include it only if present so a checkout without one still
# builds. See packaging/macos/README.md for how to generate icon.icns.
if [[ -f "$SCRIPT_DIR/icon.icns" ]]; then
	cp "$SCRIPT_DIR/icon.icns" "$CONTENTS/Resources/icon.icns"
else
	echo "note: no icon.icns found; bundling without a custom icon"
fi

# Ad-hoc code-sign the bundle. This satisfies the "must be signed" requirement
# on Apple Silicon (where unsigned binaries are killed on launch). It does NOT
# make the app trusted by Gatekeeper for distribution — that needs a Developer
# ID signature and notarization.
if command -v codesign >/dev/null 2>&1; then
	codesign --force --deep --sign - "$APP"
	echo "ad-hoc signed $APP"
fi

echo "Done: $APP"
