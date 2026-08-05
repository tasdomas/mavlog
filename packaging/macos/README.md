# macOS packaging

`mavlog` is a GUI app. A bare compiled binary double-clicked in Finder gets run
by Terminal.app (hence the stray terminal window); wrapping it in a `.app`
bundle makes macOS launch it as a proper GUI application instead.

## Build a bundle locally

From the repo root:

```sh
make bundle
```

This builds `--release` and writes `dist/mavlog.app`. Double-click it, or:

```sh
open dist/mavlog.app
```

Under the hood `make bundle` just calls `packaging/macos/bundle.sh` with the
release binary; you can invoke it directly for a custom target/output:

```sh
packaging/macos/bundle.sh target/release/mavlog dist 0.1.0
```

## Adding an app icon (optional)

Drop an `icon.icns` next to this README (`packaging/macos/icon.icns`) and it is
picked up automatically. To generate one from a 1024×1024 PNG:

```sh
mkdir mavlog.iconset
sips -z 16 16     icon.png --out mavlog.iconset/icon_16x16.png
sips -z 32 32     icon.png --out mavlog.iconset/icon_16x16@2x.png
sips -z 32 32     icon.png --out mavlog.iconset/icon_32x32.png
sips -z 64 64     icon.png --out mavlog.iconset/icon_32x32@2x.png
sips -z 128 128   icon.png --out mavlog.iconset/icon_128x128.png
sips -z 256 256   icon.png --out mavlog.iconset/icon_128x128@2x.png
sips -z 256 256   icon.png --out mavlog.iconset/icon_256x256.png
sips -z 512 512   icon.png --out mavlog.iconset/icon_256x256@2x.png
sips -z 512 512   icon.png --out mavlog.iconset/icon_512x512.png
cp icon.png                mavlog.iconset/icon_512x512@2x.png
iconutil -c icns mavlog.iconset -o packaging/macos/icon.icns
```

## Signing & notarization

`bundle.sh` ad-hoc signs the bundle (`codesign --sign -`), which is enough to
run locally on Apple Silicon. For distribution to other machines without
Gatekeeper warnings you need a Developer ID signature plus notarization:

```sh
codesign --force --deep --options runtime \
  --sign "Developer ID Application: Your Name (TEAMID)" dist/mavlog.app
ditto -c -k --keepParent dist/mavlog.app mavlog.zip
xcrun notarytool submit mavlog.zip --apple-id you@example.com \
  --team-id TEAMID --password APP_PASSWORD --wait
xcrun stapler staple dist/mavlog.app
```
