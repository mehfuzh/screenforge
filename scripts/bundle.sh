#!/bin/sh
# Assemble a macOS app bundle at target/ScreenForge.app (debug by default, `--release` for release).
set -e
cd "$(dirname "$0")/.."
PROFILE=debug
if [ "$1" = "--release" ]; then PROFILE=release; cargo build --release -p studio; else cargo build -p studio; fi
APP=target/ScreenForge.app
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "target/$PROFILE/screenforge" "$APP/Contents/MacOS/ScreenForge"
cp apps/studio/assets/icon.icns "$APP/Contents/Resources/icon.icns"
# ScreenForge's own wallpapers (and any other bundled assets) → Contents/Resources/assets
mkdir -p "$APP/Contents/Resources/assets" && cp -R assets/. "$APP/Contents/Resources/assets/"
# ffmpeg: bundle self-contained static binaries (Homebrew's link to /opt/homebrew dylibs, which
# the hardened runtime rejects and which users do not have). scripts/fetch-ffmpeg.sh caches them.
FFDIR=$(./scripts/fetch-ffmpeg.sh)
mkdir -p "$APP/Contents/Resources/bin"
cp "$FFDIR/ffmpeg" "$APP/Contents/Resources/bin/ffmpeg"
cp "$FFDIR/ffprobe" "$APP/Contents/Resources/bin/ffprobe"
VERSION=$(grep -m1 '^version' Cargo.toml | sed 's/.*"\(.*\)".*/\1/')
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleName</key><string>ScreenForge</string>
  <key>CFBundleDisplayName</key><string>ScreenForge</string>
  <key>CFBundleIdentifier</key><string>dev.screenforge.app</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleExecutable</key><string>ScreenForge</string>
  <key>CFBundleIconFile</key><string>icon.icns</string>
  <key>LSMinimumSystemVersion</key><string>14.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>LSApplicationCategoryType</key><string>public.app-category.video</string>
  <key>NSMicrophoneUsageDescription</key><string>ScreenForge records your microphone when you choose to.</string>
  <key>NSCameraUsageDescription</key><string>ScreenForge records your camera when you choose to.</string>
  <key>NSAudioCaptureUsageDescription</key><string>ScreenForge records system audio when you choose to.</string>
  <key>CFBundleDocumentTypes</key><array><dict>
    <key>CFBundleTypeExtensions</key><array><string>screenstudio</string></array>
    <key>CFBundleTypeName</key><string>Screen Studio Project</string>
    <key>CFBundleTypeRole</key><string>Editor</string>
    <key>LSTypeIsPackage</key><true/>
  </dict></array>
</dict></plist>
PLIST
# Code signing. macOS privacy permissions (Screen Recording, Input Monitoring, Microphone) are
# tied to the app's signature: an ad-hoc signed build gets a new identity on every rebuild and is
# asked again each time. Use a Developer ID from the keychain when available (CI passes
# CODESIGN_IDENTITY explicitly); otherwise sign ad-hoc with a stable identifier.
ENT=apps/studio/assets/entitlements.plist
if [ -z "$CODESIGN_IDENTITY" ] && [ "$NO_SIGN" != "1" ]; then
  CODESIGN_IDENTITY=$(security find-identity -v -p codesigning 2>/dev/null | grep "Developer ID Application" | head -1 | sed -E 's/.*"(.*)".*/\1/')
fi
if [ -n "$CODESIGN_IDENTITY" ]; then
  for f in "$APP/Contents/Resources/bin/"*; do [ -f "$f" ] && codesign --force --options runtime --timestamp --entitlements "$ENT" --sign "$CODESIGN_IDENTITY" "$f"; done
  codesign --force --options runtime --timestamp --entitlements "$ENT" --identifier dev.screenforge.app --sign "$CODESIGN_IDENTITY" "$APP"
  codesign --verify --deep --strict --verbose=2 "$APP"
  echo "signed as $CODESIGN_IDENTITY"
else
  codesign --force --identifier dev.screenforge.app --sign - "$APP"
  echo "signed ad-hoc (permissions reset on every rebuild; add a Developer ID certificate to avoid that)"
fi
echo "built $APP"
