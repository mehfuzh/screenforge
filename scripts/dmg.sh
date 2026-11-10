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
RW=target/dmg-rw.dmg
rm -f "$RW"
hdiutil create -volname "ScreenForge" -srcfolder "$STAGE" -ov -format UDRW "$RW" >/dev/null

# window layout (background, icon positions) is stored in the volume's .DS_Store, which only Finder
# writes, so mount where Finder can see it and script it. A failure here (no Finder session) leaves
# a plain but working DMG.
VOL=/Volumes/ScreenForge
[ -d "$VOL" ] && hdiutil detach "$VOL" -force >/dev/null 2>&1 || true
hdiutil attach "$RW" -noautoopen >/dev/null
mkdir "$VOL/.background" && cp apps/studio/assets/dmg-background.tiff "$VOL/.background/background.tiff"
# Finder needs a moment to see a freshly attached disk
LAYOUT=$(cat <<'OSA'
tell application "Finder"
  tell disk "ScreenForge"
    open
    set current view of container window to icon view
    set toolbar visible of container window to false
    set statusbar visible of container window to false
    set bounds of container window to {200, 120, 760, 508}
    set opts to icon view options of container window
    set arrangement of opts to not arranged
    set icon size of opts to 112
    set text size of opts to 12
    set background picture of opts to file ".background:background.tiff"
    set position of item "ScreenForge.app" to {150, 190}
    set position of item "Applications" to {410, 190}
    close
    open
    update without registering applications
    delay 1
    close
  end tell
end tell
OSA
)
for try in 1 2 3 4 5; do
  if osascript -e "$LAYOUT" >/dev/null 2>&1; then break; fi
  [ $try = 5 ] && echo "warning: could not lay out the DMG window" >&2 || sleep 2
done
# volume icon: .VolumeIcon.icns plus the custom-icon flag on the root. Done after the Finder step,
# because Finder removes both when it saves the window layout.
cp apps/studio/assets/icon.icns "$VOL/.VolumeIcon.icns"
SetFile -a V "$VOL/.VolumeIcon.icns"
SetFile -a C "$VOL"
sync
hdiutil detach "$VOL" >/dev/null || { sleep 2; hdiutil detach "$VOL" -force >/dev/null; }
hdiutil convert "$RW" -format UDZO -o "$OUT" >/dev/null
rm -f "$RW"
if [ -n "$CODESIGN_IDENTITY" ]; then codesign --force --timestamp --sign "$CODESIGN_IDENTITY" "$OUT"; fi
echo "$OUT"
