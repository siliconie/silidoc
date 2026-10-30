#!/usr/bin/env bash
# Builds Alfadoc.app (unsigned) and a zip. Run on a Mac from the repository root:
#   bash packaging/macos/build.sh
set -euo pipefail
cd "$(dirname "$0")/../.."
version=$(grep -m1 '^version' Cargo.toml | sed 's/.*"\(.*\)".*/\1/')
cargo build --release -p alfadoc-app -j 2
app="dist/Alfadoc.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp target/release/alfadoc "$app/Contents/MacOS/alfadoc"

# Icon: build an .icns from the 512px png.
iconset=$(mktemp -d)/alfadoc.iconset
mkdir -p "$iconset"
for s in 16 32 64 128 256 512; do
  sips -z $s $s packaging/icon/alfadoc.png --out "$iconset/icon_${s}x${s}.png" >/dev/null
done
iconutil -c icns "$iconset" -o "$app/Contents/Resources/alfadoc.icns"

cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleName</key><string>Alfadoc</string>
  <key>CFBundleDisplayName</key><string>Alfadoc</string>
  <key>CFBundleIdentifier</key><string>org.alfasilicon.alfadoc</string>
  <key>CFBundleVersion</key><string>$version</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>CFBundleExecutable</key><string>alfadoc</string>
  <key>CFBundleIconFile</key><string>alfadoc</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>CFBundleDocumentTypes</key><array><dict>
    <key>CFBundleTypeName</key><string>Alfadoc document</string>
    <key>CFBundleTypeExtensions</key><array><string>alfadoc</string></array>
    <key>CFBundleTypeRole</key><string>Editor</string>
  </dict></array>
</dict></plist>
PLIST

(cd dist && zip -qr "Alfadoc-$version-macos.zip" Alfadoc.app)
echo "dist/Alfadoc-$version-macos.zip"
