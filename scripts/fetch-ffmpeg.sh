#!/usr/bin/env bash
# Download self-contained (statically linked) ffmpeg + ffprobe for macOS so the app bundle does
# not depend on Homebrew. Source: https://ffmpeg.martin-riedl.de (GPL builds, arm64 and x86_64).
# Result: target/ffmpeg-static/<arch>/{ffmpeg,ffprobe}. Idempotent; reruns only if missing.
set -euo pipefail
cd "$(dirname "$0")/.."
ARCH=${FFMPEG_ARCH:-$(uname -m)}            # arm64 | x86_64
case "$ARCH" in arm64|x86_64) ;; *) echo "unsupported arch $ARCH"; exit 1 ;; esac
DEST="target/ffmpeg-static/$ARCH"
if [ -x "$DEST/ffmpeg" ] && [ -x "$DEST/ffprobe" ]; then echo "$DEST"; exit 0; fi
TMP=$(mktemp -d); trap 'rm -rf "$TMP"' EXIT
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
