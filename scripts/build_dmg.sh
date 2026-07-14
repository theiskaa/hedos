#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."

APP=dist/Hedos.app
DMG=dist/Hedos.dmg
BG_CACHE=dist/.dmg-assets
BG="$BG_CACHE/background.png"

if [ "${REBUILD:-1}" = "1" ] || [ ! -d "$APP" ]; then
    echo "==> Building the app bundle"
    ./scripts/build_app.sh
fi

echo "==> Preparing the DMG toolchain (pillow + dmgbuild)"
mkdir -p "$BG_CACHE"
VENV=$(mktemp -d -t hedos-dmg)
uv venv "$VENV/venv" --python 3.12 >/dev/null
uv pip install --python "$VENV/venv/bin/python" pillow dmgbuild >/dev/null

echo "==> Rendering the background"
"$VENV/venv/bin/python" scripts/dmg_background.py "$BG"

echo "==> Building a styled, compressed DMG (UDBZ / bzip2)"
hdiutil detach "/Volumes/Hedos" >/dev/null 2>&1 || true
rm -f "$DMG"
"$VENV/venv/bin/dmgbuild" -s scripts/dmg_settings.py \
    -D app="$APP" -D background="$BG" "Hedos" "$DMG" >/dev/null
rm -rf "$VENV"

APP_MB=$(du -sm "$APP" | cut -f1)
DMG_MB=$(du -sm "$DMG" | cut -f1)
SAVED=$(( 100 - (DMG_MB * 100 / APP_MB) ))
echo "==> DMG: $(du -sh "$DMG" | cut -f1)  (${SAVED}% smaller than the ${APP_MB} MB bundle)"
echo "==> SHA-256:"
shasum -a 256 "$DMG"
