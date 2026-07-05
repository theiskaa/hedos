#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."

swift build -c release

APP=dist/Hedos.app
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"

RELEASE=$(swift build -c release --show-bin-path)

cp "$RELEASE/Hedos" "$APP/Contents/MacOS/Hedos"
for bundle in "$RELEASE"/*.bundle; do
    cp -R "$bundle" "$APP/Contents/Resources/"
    cp -R "$bundle" "$APP/Contents/MacOS/"
done
for framework in "$RELEASE"/*.framework; do
    [ -e "$framework" ] || continue
    cp -R "$framework" "$APP/Contents/MacOS/"
done
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
    <string>0.1.0</string>
    <key>CFBundleVersion</key>
    <string>1</string>
    <key>LSMinimumSystemVersion</key>
    <string>15.0</string>
    <key>NSHighResolutionCapable</key>
    <true/>
    <key>NSHumanReadableCopyright</key>
    <string>MIT License</string>
</dict>
</plist>
PLIST

IDENTITY=$(security find-identity -v -p codesigning 2>/dev/null | grep "Developer ID Application" | head -1 | awk -F'"' '{print $2}')
if [ -n "$IDENTITY" ]; then
    codesign --force --options runtime --sign "$IDENTITY" "$APP"
    echo "signed: $IDENTITY"
else
    codesign --force --options runtime --sign - "$APP"
    echo "signed: ad-hoc"
fi

echo "built: $APP"
