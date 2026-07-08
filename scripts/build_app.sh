#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."

swift build -c release

APP=dist/Hedos.app
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources" "$APP/Contents/Frameworks"

RELEASE=$(swift build -c release --show-bin-path)

cp "$RELEASE/Hedos" "$APP/Contents/MacOS/Hedos"
for bundle in "$RELEASE"/*.bundle; do
    [ -e "$bundle" ] || continue
    cp -R "$bundle" "$APP/Contents/Resources/"
done
HAS_FRAMEWORKS=0
for framework in "$RELEASE"/*.framework; do
    [ -e "$framework" ] || continue
    cp -R "$framework" "$APP/Contents/Frameworks/"
    HAS_FRAMEWORKS=1
done
if [ "$HAS_FRAMEWORKS" = 1 ]; then
    install_name_tool -add_rpath "@executable_path/../Frameworks" "$APP/Contents/MacOS/Hedos"
else
    rmdir "$APP/Contents/Frameworks"
fi
cp Sources/Hedos/Resources/Hedos.icns "$APP/Contents/Resources/Hedos.icns"

cat > "$APP/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleExecutable</key>
    <string>Hedos</string>
    <key>CFBundleIdentifier</key>
    <string>dev.theiskaa.hedos</string>
    <key>CFBundleName</key>
    <string>Hedos</string>
    <key>CFBundleDisplayName</key>
    <string>Hedos</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleIconFile</key>
    <string>Hedos</string>
    <key>CFBundleShortVersionString</key>
    <string>0.0.0</string>
    <key>CFBundleVersion</key>
    <string>1</string>
    <key>LSMinimumSystemVersion</key>
    <string>26.0</string>
    <key>NSHighResolutionCapable</key>
    <true/>
    <key>NSHumanReadableCopyright</key>
    <string>MIT License</string>
    <key>NSMicrophoneUsageDescription</key>
    <string>Hedos records from the microphone only while you dictate, and the audio never leaves this Mac.</string>
</dict>
</plist>
PLIST

IDENTITY=$(security find-identity -v -p codesigning 2>/dev/null \
    | awk -F'"' '/Developer ID Application/ {print $2; exit}' || true)

ENTITLEMENTS="$(mktemp -t hedos-entitlements).plist"
cat > "$ENTITLEMENTS" <<'ENT'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>com.apple.security.virtualization</key>
    <true/>
</dict>
</plist>
ENT

sign_bundle() {
    for framework in "$APP"/Contents/Frameworks/*.framework; do
        [ -e "$framework" ] || continue
        codesign --force "$@" "$framework"
    done
    codesign --force "$@" --entitlements "$ENTITLEMENTS" "$APP"
}

if [ -n "$IDENTITY" ]; then
    sign_bundle --options runtime --sign "$IDENTITY"
    echo "signed: $IDENTITY"
else
    sign_bundle --sign -
    echo "signed: ad-hoc"
fi
rm -f "$ENTITLEMENTS"

codesign --verify --deep --strict "$APP"
echo "built: $APP"
