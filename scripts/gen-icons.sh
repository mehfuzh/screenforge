#!/bin/sh
# Render every app icon asset from apps/studio/assets/icon.svg (needs rsvg-convert and ImageMagick).
#   icon-{256,512,1024}.png   macOS-style tile with margin and drop shadow
#   icon.icns                 each size rendered from the vector, not downscaled from 1024
#   icon-tile-256.png         Windows window/tray icon (same tile as icon.ico)
#   tray-template.png         macOS menu-bar template (from tray-template.svg, 44 px for 22 pt)
#   icon.ico                  Windows: no drop shadow, tile fills the frame, 16..256 px frames
#   installer/*.bmp           Inno Setup wizard panel and header images
#   dmg-background.tiff       DMG window background, 1x + 2x (from dmg-background.svg)
set -e
cd "$(dirname "$0")/../apps/studio/assets"
TMP=$(mktemp -d); trap 'rm -rf "$TMP"' EXIT

render() { rsvg-convert -w "$2" -h "$2" "$1" -o "$3"; }

# Windows/installer variant: drop the shadow and crop to the tile (small margin for antialiasing)
sed -e '/filter="url(#shadow)"/d' -e 's/viewBox="0 0 1024 1024"/viewBox="88 88 848 848"/' icon.svg > "$TMP/tile.svg"

for s in 256 512 1024; do render icon.svg $s icon-$s.png; done

mkdir "$TMP/icon.iconset"
for s in 16 32 128 256 512; do
  render icon.svg $s "$TMP/icon.iconset/icon_${s}x${s}.png"
  render icon.svg $((s * 2)) "$TMP/icon.iconset/icon_${s}x${s}@2x.png"
done
iconutil -c icns "$TMP/icon.iconset" -o icon.icns

ICO=""
for s in 16 20 24 32 40 48 64 96 128 256; do render "$TMP/tile.svg" $s "$TMP/ico-$s.png"; ICO="$ICO $TMP/ico-$s.png"; done
# shellcheck disable=SC2086
magick $ICO icon.ico
cp "$TMP/ico-256.png" icon-tile-256.png

# Inno Setup: 24-bit BMPs. Wizard panel = dark backdrop with the tile; header = tile on white.
render "$TMP/tile.svg" 1024 "$TMP/tile-1024.png"
for wh in 164x314 246x459 328x628; do
  w=${wh%x*}; h=${wh#*x}; t=$((w * 46 / 100))
  magick -size ${w}x${h} 'xc:rgb(23,22,37)' \( "$TMP/tile-1024.png" -resize ${t}x${t} \) -gravity center -composite \
    -type TrueColor BMP3:installer/wizard-$w.bmp
done
for w in 55 83 110; do
  t=$((w * 84 / 100))
  magick -size ${w}x${w} xc:white \( "$TMP/tile-1024.png" -resize ${t}x${t} \) -gravity center -composite \
    -type TrueColor BMP3:installer/small-$w.bmp
done
render tray-template.svg 44 tray-template.png
rsvg-convert -w 560 -h 360 dmg-background.svg -o "$TMP/bg.png"
rsvg-convert -w 1120 -h 720 dmg-background.svg -o "$TMP/bg@2x.png"
tiffutil -cathidpicheck "$TMP/bg.png" "$TMP/bg@2x.png" -out dmg-background.tiff >/dev/null 2>&1
echo "icons regenerated in $(pwd)"
