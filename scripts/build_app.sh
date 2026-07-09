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
for artifact in Assets.car hedos.icns; do
    if [ ! -f "Sources/Hedos/Resources/$artifact" ]; then
        echo "error: missing Sources/Hedos/Resources/$artifact" >&2
        exit 1
    fi
    cp "Sources/Hedos/Resources/$artifact" "$APP/Contents/Resources/$artifact"
done

MLX_VERSION=$(jq -r '.pins[] | select(.identity=="mlx-swift") | .state.version' Package.resolved)
if [ -z "$MLX_VERSION" ] || [ "$MLX_VERSION" = "null" ]; then
    echo "error: could not resolve mlx-swift version from Package.resolved" >&2
    exit 1
fi
METALLIB_CACHE="dist/.mlx-metallib-cache/$MLX_VERSION"
METALLIB="$METALLIB_CACHE/mlx.metallib"
if [ ! -f "$METALLIB" ]; then
    echo "fetching mlx==$MLX_VERSION metallib via uv"
    VENV=$(mktemp -d -t hedos-mlx-metallib)
    uv venv "$VENV/venv" --python 3.12 >/dev/null
    uv pip install --python "$VENV/venv/bin/python" "mlx==$MLX_VERSION" >/dev/null
    FOUND=$(find "$VENV/venv" -path '*/mlx/lib/mlx.metallib' | head -1)
    if [ -z "$FOUND" ]; then
        echo "error: mlx==$MLX_VERSION did not provide mlx.metallib" >&2
        rm -rf "$VENV"
        exit 1
    fi
    mkdir -p "$METALLIB_CACHE"
    cp "$FOUND" "$METALLIB"
    rm -rf "$VENV"
fi
cp "$METALLIB" "$APP/Contents/MacOS/mlx.metallib"

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
    <string>hedos</string>
    <key>CFBundleIconName</key>
    <string>hedos</string>
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
    codesign --force "$@" "$APP/Contents/MacOS/mlx.metallib"
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
