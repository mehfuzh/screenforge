#!/bin/sh
# Create target/ScreenForge-<version>.dmg from target/ScreenForge.app (run scripts/bundle.sh first).
set -e
cd "$(dirname "$0")/.."
VERSION=$(grep -m1 '^version' Cargo.toml | sed 's/.*"\(.*\)".*/\1/')
OUT="target/ScreenForge-$VERSION.dmg"
STAGE=target/dmg-stage
rm -rf "$STAGE" "$OUT"; mkdir -p "$STAGE"
cp -R target/ScreenForge.app "$STAGE/"
ln -s /Applications "$STAGE/Applications"
hdiutil create -volname "ScreenForge" -srcfolder "$STAGE" -ov -format UDZO "$OUT" >/dev/null
if [ -n "$CODESIGN_IDENTITY" ]; then codesign --force --timestamp --sign "$CODESIGN_IDENTITY" "$OUT"; fi
echo "$OUT"
