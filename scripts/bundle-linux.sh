#!/usr/bin/env bash
# Linux packages: target/ScreenForge-<version>-linux-x86_64.tar.gz (portable folder) and
# target/screenforge_<version>_amd64.deb (installs to /opt/screenforge with a `screenforge`
# command, desktop entry and icon). Both carry a static ffmpeg + ffprobe next to the executable
# (scripts/fetch-ffmpeg.sh picks a build that includes the X11 grabber), so neither needs the
# distro's ffmpeg.
#
#   scripts/bundle-linux.sh            # release build + both packages
set -euo pipefail
cd "$(dirname "$0")/.."
VERSION=$(grep -m1 '^version = ' Cargo.toml | sed -E 's/version = "(.*)"/\1/')
ARCH_DEB=amd64; ARCH=x86_64
cargo build --release -p studio
BIN=target/release/screenforge

# portable folder: <dir>/screenforge + assets/ (the app finds assets next to its executable)
DIR=target/linux/ScreenForge
rm -rf target/linux && mkdir -p "$DIR"
cp "$BIN" "$DIR/screenforge" && strip "$DIR/screenforge" || true
FFDIR=$(./scripts/fetch-ffmpeg.sh)
cp "$FFDIR/ffmpeg" "$FFDIR/ffprobe" "$DIR/"
cp "$FFDIR/LICENSE.txt" "$DIR/FFMPEG-LICENSE.txt"
cp -R assets "$DIR/assets"
cp apps/studio/assets/icon-256.png "$DIR/screenforge.png"
cat > "$DIR/screenforge.desktop" <<DESK
[Desktop Entry]
Type=Application
Name=ScreenForge
Comment=Screen recorder and timeline editor
Exec=screenforge %F
Icon=screenforge
Terminal=false
Categories=AudioVideo;Video;Recorder;
MimeType=inode/directory;
StartupWMClass=screenforge
DESK
cat > "$DIR/README.txt" <<TXT
ScreenForge $VERSION for Linux (x86_64)

Run ./screenforge. ffmpeg and ffprobe are included (static GPL builds from
https://github.com/BtbN/FFmpeg-Builds; see FFMPEG-LICENSE.txt).
Recording works in X11 sessions; Wayland recording is not supported yet. Pick an "on Xorg"
session at the login screen; GNOME 49 and later have none, so install an X11 desktop such as
Xfce (sudo apt install xfce4 xserver-xorg) and log in to "Xfce Session".
Editing and exporting existing projects work everywhere.
TXT
TAR="target/ScreenForge-$VERSION-linux-$ARCH.tar.gz"
tar -C target/linux -czf "$TAR" ScreenForge

# .deb
ROOT=target/linux/deb
mkdir -p "$ROOT/DEBIAN" "$ROOT/opt" "$ROOT/usr/bin" "$ROOT/usr/share/applications" "$ROOT/usr/share/icons/hicolor/256x256/apps"
cp -R "$DIR" "$ROOT/opt/screenforge"
rm -f "$ROOT/opt/screenforge/README.txt"
ln -s /opt/screenforge/screenforge "$ROOT/usr/bin/screenforge"
mv "$ROOT/opt/screenforge/screenforge.desktop" "$ROOT/usr/share/applications/screenforge.desktop"
mv "$ROOT/opt/screenforge/screenforge.png" "$ROOT/usr/share/icons/hicolor/256x256/apps/screenforge.png"
SIZE=$(du -sk "$ROOT" | cut -f1)
cat > "$ROOT/DEBIAN/control" <<CTRL
Package: screenforge
Version: $VERSION
Section: video
Priority: optional
Architecture: $ARCH_DEB
Installed-Size: $SIZE
Depends: libgtk-3-0 | libgtk-3-0t64, libasound2 | libasound2t64, libxtst6, libxkbcommon0, libx11-6, libxcursor1, libxrandr2, libxi6, libpipewire-0.3-0 | libpipewire-0.3-0t64
Maintainer: Mehfuz Hossain <mehfuz@smartloop.ai>
Homepage: https://screenforge.studio
Description: Screen recorder and timeline editor
 Record the screen and edit on a timeline: automatic zooms, smooth cursor,
 backgrounds, music and a separate audio track. Projects use the .screenstudio
 package format.
CTRL
DEB="target/screenforge_${VERSION}_${ARCH_DEB}.deb"
dpkg-deb -Zxz --root-owner-group --build "$ROOT" "$DEB" >/dev/null
# stable names so releases/latest/download/... always points at the newest build
cp "$TAR" target/ScreenForge-linux-x86_64.tar.gz
cp "$DEB" target/screenforge_amd64.deb
echo "$TAR"
echo "$DEB"
