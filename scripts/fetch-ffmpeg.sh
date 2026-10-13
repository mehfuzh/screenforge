#!/usr/bin/env bash
# Download self-contained (statically linked) ffmpeg + ffprobe so the packages do not depend on
# Homebrew or the distro's ffmpeg.
#   macOS: https://ffmpeg.martin-riedl.de (GPL builds, arm64 and x86_64)
#          → target/ffmpeg-static/<arch>/{ffmpeg,ffprobe}
#   Linux: https://github.com/BtbN/FFmpeg-Builds (GPL builds, newest release series, x86_64; these
#          include x11grab, which many static builds leave out)
#          → target/ffmpeg-static/linux-<arch>/{ffmpeg,ffprobe,LICENSE.txt}
# Idempotent; reruns only if missing. Prints the directory on stdout.
set -euo pipefail
cd "$(dirname "$0")/.."
ARCH=${FFMPEG_ARCH:-$(uname -m)}            # arm64 | x86_64
TMP=$(mktemp -d); trap 'rm -rf "$TMP"' EXIT

if [ "$(uname -s)" = Linux ]; then
  [ "$ARCH" = x86_64 ] || { echo "unsupported arch $ARCH"; exit 1; }
  DEST="target/ffmpeg-static/linux-$ARCH"
  if [ -x "$DEST/ffmpeg" ] && [ -x "$DEST/ffprobe" ]; then echo "$DEST"; exit 0; fi
  # BtbN renames release assets as ffmpeg versions come and go: pick the newest nX.Y series
  # (override with FFMPEG_LINUX_ASSET=ffmpeg-n9.0-latest-linux64-gpl-9.0.tar.xz)
  ASSET=${FFMPEG_LINUX_ASSET:-$(curl -fsSL ${GH_TOKEN:+-H "Authorization: Bearer $GH_TOKEN"} \
      https://api.github.com/repos/BtbN/FFmpeg-Builds/releases/latest \
    | grep -oE '"name": *"ffmpeg-n[0-9.]+-latest-linux64-gpl-[0-9.]+\.tar\.xz"' \
    | sed -E 's/.*"(ffmpeg-[^"]+)"/\1/' | sort -V | tail -1)}
  [ -n "$ASSET" ] || { echo "no linux64-gpl release asset found" >&2; exit 1; }
  url="https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/$ASSET"
  echo "→ ffmpeg + ffprobe (linux $ARCH) from $url" >&2
  curl -fsSL "$url" -o "$TMP/ff.tar.xz"
  tar -C "$TMP" -xJf "$TMP/ff.tar.xz"
  SRC=$(dirname "$(find "$TMP" -type f -path '*/bin/ffmpeg' | head -1)")
  for tool in ffmpeg ffprobe; do
    bin="$SRC/$tool"
    [ -x "$bin" ] || { echo "no $tool binary in the archive" >&2; exit 1; }
    # must not link anything beyond glibc
    if ldd "$bin" | grep -v -E "linux-vdso|ld-linux|/lib(c|m|dl|rt|pthread|mvec|gcc_s)\.so" | grep -q .; then
      echo "$tool is not self-contained:" >&2; ldd "$bin" >&2; exit 1
    fi
  done
  "$SRC/ffmpeg" -hide_banner -devices 2>/dev/null | grep -q x11grab || { echo "ffmpeg build has no x11grab" >&2; exit 1; }
  mkdir -p "$DEST"
  cp "$SRC/ffmpeg" "$SRC/ffprobe" "$DEST/" && chmod +x "$DEST/ffmpeg" "$DEST/ffprobe"
  cp "$SRC/../LICENSE.txt" "$DEST/LICENSE.txt"
  "$DEST/ffmpeg" -version | head -1 >&2
  echo "$DEST"
  exit 0
fi

case "$ARCH" in arm64|x86_64) ;; *) echo "unsupported arch $ARCH"; exit 1 ;; esac
DEST="target/ffmpeg-static/$ARCH"
if [ -x "$DEST/ffmpeg" ] && [ -x "$DEST/ffprobe" ]; then echo "$DEST"; exit 0; fi
for tool in ffmpeg ffprobe; do
  url="https://ffmpeg.martin-riedl.de/redirect/latest/macos/$ARCH/release/$tool.zip"
  echo "→ $tool ($ARCH) from $url" >&2
  curl -fsSL "$url" -o "$TMP/$tool.zip"
  mkdir -p "$TMP/$tool" && unzip -q "$TMP/$tool.zip" -d "$TMP/$tool"
  bin=$(find "$TMP/$tool" -type f -name "$tool" | head -1)
  [ -n "$bin" ] || { echo "no $tool binary in the archive" >&2; exit 1; }
  # must not link anything outside the OS
  if otool -L "$bin" | tail -n +2 | grep -v -E "^\s+(/usr/lib/|/System/)" | grep -q .; then
    echo "$tool is not self-contained:" >&2; otool -L "$bin" >&2; exit 1
  fi
  mkdir -p "$DEST" && cp "$bin" "$DEST/$tool" && chmod +x "$DEST/$tool"
done
"$DEST/ffmpeg" -version | head -1 >&2
echo "$DEST"
