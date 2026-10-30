#!/usr/bin/env bash
# Builds a release binary and a tar.gz with the desktop entry, icon and file type.
# Run from the repository root:  bash packaging/linux/build.sh
set -euo pipefail
cd "$(dirname "$0")/../.."
version=$(grep -m1 '^version' Cargo.toml | sed 's/.*"\(.*\)".*/\1/')
cargo build --release -p alfadoc-app -j 2
stage="dist/alfadoc-$version-linux-x86_64"
rm -rf "$stage"
mkdir -p "$stage/bin" "$stage/share/applications" "$stage/share/mime/packages" "$stage/share/icons/hicolor/512x512/apps"
cp target/release/alfadoc "$stage/bin/"
cp packaging/linux/alfadoc.desktop "$stage/share/applications/"
cp packaging/linux/alfadoc-mime.xml "$stage/share/mime/packages/"
cp packaging/icon/alfadoc.png "$stage/share/icons/hicolor/512x512/apps/alfadoc.png"
tar -C dist -czf "dist/alfadoc-$version-linux-x86_64.tar.gz" "alfadoc-$version-linux-x86_64"
echo "dist/alfadoc-$version-linux-x86_64.tar.gz"
