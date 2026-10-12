#!/usr/bin/env bash
# Build, sign, notarize and publish a release from this Mac. No CI secrets, no certificate
# export: codesign uses the Developer ID in the login keychain, notarytool uses the keychain
# profile created by scripts/setup-release.sh.
#
#   scripts/release.sh 0.1.0          # builds, signs, notarizes, creates GitHub release v0.1.0
#   scripts/release.sh 0.1.0 --draft  # same, but the GitHub release stays a draft
set -euo pipefail
cd "$(dirname "$0")/.."

VERSION=${1:?usage: scripts/release.sh <version> [--draft]}
VERSION=${VERSION#v}
DRAFT=""; [ "${2:-}" = "--draft" ] && DRAFT="--draft"
REPO=${REPO:-mehfuzh/screenforge}
PROFILE=${NOTARY_PROFILE:-screenforge}

for t in gh xcrun codesign hdiutil; do command -v $t >/dev/null || { echo "missing: $t"; exit 1; }; done
IDENTITY=$(security find-identity -v -p codesigning | grep "Developer ID Application" | head -1 | sed -E 's/.*"(.*)".*/\1/')
[ -n "$IDENTITY" ] || { echo "no Developer ID Application certificate in the keychain"; exit 1; }
xcrun notarytool history --keychain-profile "$PROFILE" >/dev/null 2>&1 || { echo "no notarytool profile '$PROFILE'. Run scripts/setup-release.sh first."; exit 1; }
if [ -n "$(git status --porcelain)" ]; then echo "working tree is not clean; commit first"; exit 1; fi

echo "→ Version $VERSION in Cargo.toml"
if ! grep -q "^version = \"$VERSION\"" Cargo.toml; then
  sed -i '' -E "s/^version = \"[0-9]+\.[0-9]+\.[0-9]+\"/version = \"$VERSION\"/" Cargo.toml
  cargo update -w -q
  git add Cargo.toml Cargo.lock
  git -c user.name="Mehfuz Hossain" -c user.email="mehfuz@smartloop.ai" commit -q -m "Release $VERSION"
fi

echo "→ Tests"
cargo test --workspace -q 2>&1 | grep -E "test result: FAILED|panicked" && { echo "tests failed"; exit 1; } || true

echo "→ Release build + bundle (signed with $IDENTITY)"
CODESIGN_IDENTITY="$IDENTITY" ./scripts/bundle.sh --release
codesign --verify --deep --strict target/ScreenForge.app && echo "  signature ok"

echo "→ Notarizing the app (this takes a few minutes)"
ditto -c -k --keepParent target/ScreenForge.app target/ScreenForge-notarize.zip
xcrun notarytool submit target/ScreenForge-notarize.zip --keychain-profile "$PROFILE" --wait
xcrun stapler staple target/ScreenForge.app
rm -f target/ScreenForge-notarize.zip
spctl --assess --type execute -vv target/ScreenForge.app 2>&1 | sed 's/^/  /'

echo "→ DMG + zip"
DMG=$(VERSION="$VERSION" ./scripts/dmg.sh)
xcrun notarytool submit "$DMG" --keychain-profile "$PROFILE" --wait >/dev/null
xcrun stapler staple "$DMG"
ZIP="target/ScreenForge-$VERSION-macOS.zip"
ditto -c -k --keepParent target/ScreenForge.app "$ZIP"
# stable names so https://github.com/<repo>/releases/latest/download/ScreenForge.dmg always works
cp "$DMG" target/ScreenForge.dmg && cp "$ZIP" target/ScreenForge-macOS.zip

echo "→ Tag + GitHub release v$VERSION"
git tag -f "v$VERSION" && git push -q origin main "v$VERSION"
NOTES=$(git log --format='- %s' "$(git describe --tags --abbrev=0 "v$VERSION^" 2>/dev/null || git rev-list --max-parents=0 HEAD)..v$VERSION" | grep -v "^- Release " | head -40)
gh release create "v$VERSION" "$DMG" "$ZIP" target/ScreenForge.dmg target/ScreenForge-macOS.zip -R "$REPO" $DRAFT --title "ScreenForge $VERSION" --notes "$NOTES"
echo "  https://github.com/$REPO/releases/tag/v$VERSION"
