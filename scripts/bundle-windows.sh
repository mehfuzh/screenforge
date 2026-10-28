#!/usr/bin/env bash
# Windows packages:
#   target/ScreenForge-<version>-windows-x86_64-setup.exe   installer (Inno Setup: per-user, Start
#                                                          menu, uninstaller, .screenforge association)
#   target/ScreenForge-<version>-windows-x86_64.zip         portable folder
# Both carry screenforge.exe, a static ffmpeg + ffprobe (scripts/fetch-ffmpeg.sh) and the bundled
# assets. Runs under Git Bash / MSYS on Windows (and on the GitHub windows runners, which ship
# Inno Setup). Without ISCC on the machine only the zip is built.
#
#   scripts/bundle-windows.sh            # release build + installer + zip
set -euo pipefail
cd "$(dirname "$0")/.."
VERSION=$(grep -m1 '^version = ' Cargo.toml | sed -E 's/version = "(.*)"/\1/')
ARCH=x86_64
cargo build --release -p studio
BIN=target/release/screenforge.exe

DIR=target/windows/ScreenForge
rm -rf target/windows && mkdir -p "$DIR"
cp "$BIN" "$DIR/screenforge.exe"
FFDIR=$(./scripts/fetch-ffmpeg.sh)
cp "$FFDIR/ffmpeg.exe" "$FFDIR/ffprobe.exe" "$DIR/"
cp "$FFDIR/LICENSE.txt" "$DIR/FFMPEG-LICENSE.txt"
# Authenticode (Azure Trusted Signing); no-op without SF_SIGNTOOL, see scripts/sign-windows.sh
./scripts/sign-windows.sh "$DIR/screenforge.exe" "$DIR/ffmpeg.exe" "$DIR/ffprobe.exe"
cp -R assets "$DIR/assets"
cp LICENSE "$DIR/LICENSE.txt"
cat > "$DIR/README.txt" <<TXT
ScreenForge $VERSION for Windows (x86_64)

Unzip anywhere and run screenforge.exe. ffmpeg and ffprobe are included (static GPL builds
from https://github.com/BtbN/FFmpeg-Builds; see FFMPEG-LICENSE.txt).
Needs Windows 10 version 2004 or later. Recording uses the Desktop Duplication API; the
cursor, clicks and keystrokes are recorded separately so the editor can smooth and zoom them.
Windows SmartScreen may warn about an unsigned download: choose "More info" → "Run anyway".
TXT

ZIP="target/ScreenForge-$VERSION-windows-$ARCH.zip"
rm -f "$ZIP"
if command -v 7z >/dev/null 2>&1; then
  (cd target/windows && 7z a -tzip -mx=7 "../../$ZIP" ScreenForge >/dev/null)
else
  powershell.exe -NoProfile -Command "Compress-Archive -Path 'target/windows/ScreenForge' -DestinationPath '$ZIP' -CompressionLevel Optimal -Force"
fi
# stable name so releases/latest/download/... always points at the newest build
cp "$ZIP" "target/ScreenForge-windows-$ARCH.zip"
echo "$ZIP"

# installer
ISCC=${ISCC:-}
for c in "$ISCC" "/c/Program Files (x86)/Inno Setup 6/ISCC.exe" "$LOCALAPPDATA/Programs/Inno Setup 6/ISCC.exe" "$(command -v iscc || true)" "$(command -v ISCC || true)"; do
  [ -n "$c" ] && [ -x "$c" ] && ISCC=$c && break
done
if [ -n "$ISCC" ] && [ -x "$ISCC" ]; then
  SETUP="target/ScreenForge-$VERSION-windows-$ARCH-setup.exe"
  SIGNARGS=()
  if [ -n "${SF_SIGNTOOL:-}" ]; then
    # The signtool command line lives in the .iss (SignTool= directive, same invocation as
    # scripts/sign-windows.sh); /Ssfsign=$p makes Inno run that line as-is. Only plain paths are
    # passed here: ISCC splits arguments the Delphi way, and embedded quotes break it.
    SIGNARGS=('/Ssfsign=$p' "/DSignTool=$(cygpath -w "$SF_SIGNTOOL")" "/DSignDlib=$(cygpath -w "$SF_SIGN_DLIB")" "/DSignMeta=$(cygpath -w "$SF_SIGN_METADATA")")
  fi
  MSYS2_ARG_CONV_EXCL="*" "$ISCC" /Q "/DAppVersion=$VERSION" "/DSourceDir=$(cygpath -w "$PWD/$DIR")" "/DOutDir=$(cygpath -w "$PWD/target")" "${SIGNARGS[@]}" scripts/windows-installer.iss
  [ -f "$SETUP" ] || { echo "installer was not written" >&2; exit 1; }
  cp "$SETUP" "target/ScreenForge-windows-$ARCH-setup.exe"
  echo "$SETUP"
else
  echo "Inno Setup (ISCC) not found; skipped the installer (winget install JRSoftware.InnoSetup)" >&2
fi
