#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."

# One-command release: bump version -> build signed DMG -> notarize + staple ->
# checksum -> pin the cask -> commit -> push -> cut the GitHub release (-> update tap).
#
#   ./scripts/release.sh 0.1.1
#
# Env:
#   NOTARY_PROFILE   notarytool keychain profile (default: AC_NOTARY)
#   TAP_DIR          path to a local homebrew-tap checkout; if set, the cask is
#                    copied in, committed, and pushed automatically.

NOTARY_PROFILE="${NOTARY_PROFILE:-AC_NOTARY}"
CASK="packaging/homebrew/Casks/hedos.rb"
DMG="dist/Hedos.dmg"

CURRENT=$(perl -ne 'print $1 if /^\s*version\s+"([^"]*)"/' "$CASK" 2>/dev/null)
VERSION="${1:-}"
VERSION="${VERSION#v}"
if [ -z "$VERSION" ]; then
    echo "Current release: v${CURRENT:-0.0.0}"
    read -r -p "New version to release (e.g. 0.1.1, 0.2.0, 1.0.0): " VERSION
    VERSION="${VERSION#v}"
fi
if ! printf '%s' "$VERSION" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$'; then
    echo "invalid version '$VERSION' — expected N.N.N, e.g. 0.2.0" >&2
    exit 1
fi
TAG="v$VERSION"

echo "==> Preflight"
command -v gh >/dev/null || { echo "gh not found — install the GitHub CLI" >&2; exit 1; }
command -v jq >/dev/null || { echo "jq not found — brew install jq" >&2; exit 1; }
command -v uv >/dev/null || { echo "uv not found — https://docs.astral.sh/uv/" >&2; exit 1; }
gh auth status >/dev/null 2>&1 || { echo "gh is not authenticated (gh auth login)" >&2; exit 1; }
security find-identity -v -p codesigning | grep -q "Developer ID Application" \
    || { echo "no 'Developer ID Application' certificate in your keychain" >&2; exit 1; }
if gh release view "$TAG" >/dev/null 2>&1; then
    echo "release $TAG already exists on GitHub" >&2; exit 1
fi
if [ "$(git rev-parse --abbrev-ref HEAD)" != "main" ]; then
    echo "not on main — checkout main before releasing" >&2; exit 1
fi
if ! git diff --quiet || ! git diff --cached --quiet; then
    echo "working tree not clean — commit or stash first so the release matches the pushed source" >&2
    exit 1
fi

echo "==> This will build, notarize, and publish hedos $TAG (current: v${CURRENT:-0.0.0})"
read -r -p "    Continue? [y/N] " REPLY
case "$REPLY" in y | Y | yes | YES) ;; *) echo "aborted"; exit 1 ;; esac

trap 'git checkout -- scripts/build_app.sh "$CASK" 2>/dev/null || true' ERR

echo "==> Bumping version to $VERSION"
perl -0pi -e \
    's/(<key>CFBundleShortVersionString<\/key>\s*<string>)[^<]*(<\/string>)/${1}'"$VERSION"'${2}/' \
    scripts/build_app.sh
perl -pi -e 's/(^\s*version\s+")[^"]*(")/${1}'"$VERSION"'${2}/' "$CASK"

echo "==> Building the signed, styled DMG"
./scripts/build_dmg.sh

echo "==> Confirming Developer ID signature (not ad-hoc)"
codesign -dvv dist/Hedos.app 2>&1 | grep -q "Developer ID Application" \
    || { echo "app is ad-hoc signed, not Developer ID — aborting before notarization" >&2; exit 1; }

echo "==> Notarizing (this waits for Apple)"
xcrun notarytool submit "$DMG" --keychain-profile "$NOTARY_PROFILE" --wait
xcrun stapler staple "$DMG"
xcrun stapler validate "$DMG" \
    || { echo "notarization ticket is not stapled to the DMG — aborting" >&2; exit 1; }

echo "==> Checksum"
SHA=$(shasum -a 256 "$DMG" | awk '{print $1}')
printf '%s  Hedos.dmg\n' "$SHA" | tee "$DMG.sha256"
perl -pi -e 's/(^\s*sha256\s+")[^"]*(")/${1}'"$SHA"'${2}/' "$CASK"

echo "==> Committing the version bump"
git add scripts/build_app.sh "$CASK"
if ! git diff --cached --quiet; then
    git commit -q -m "chore(release): hedos $TAG"
fi
git push origin main
trap - ERR

echo "==> Cutting the GitHub release"
NOTES=$(mktemp)
printf 'hedos %s.\n\nDownload `Hedos.dmg` below and drag it into Applications, or install with Homebrew:\n\n    brew install --cask theiskaa/tap/hedos\n\nRequires Apple Silicon and macOS 26 (Tahoe) or later. Signed with a Developer ID and notarized.\n\nSHA-256 (`Hedos.dmg`): `%s`\n' \
    "$TAG" "$SHA" > "$NOTES"
if ! gh release create "$TAG" "$DMG" "$DMG.sha256" --target main --title "$TAG" --notes-file "$NOTES"; then
    rm -f "$NOTES"
    echo "release creation failed — the version commit is already pushed; re-run ./scripts/release.sh $VERSION to finish (it is idempotent)" >&2
    exit 1
fi
rm -f "$NOTES"

echo "==> Released: https://github.com/theiskaa/hedos/releases/tag/$TAG"

if [ -n "${TAP_DIR:-}" ]; then
    echo "==> Updating the Homebrew tap at $TAP_DIR"
    if (
        set -e
        cp "$CASK" "$TAP_DIR/Casks/hedos.rb"
        git -C "$TAP_DIR" add Casks/hedos.rb
        git -C "$TAP_DIR" diff --cached --quiet && exit 0
        git -C "$TAP_DIR" commit -q -m "hedos $VERSION"
        git -C "$TAP_DIR" push
    ); then
        echo "    tap updated — brew install --cask theiskaa/tap/hedos"
    else
        echo "    tap update failed — update it manually: cp $CASK <tap>/Casks/hedos.rb; commit; push"
    fi
else
    echo "==> Tap not updated (set TAP_DIR=/path/to/homebrew-tap to automate it)."
    echo "    Manually: cp $CASK <tap>/Casks/hedos.rb && commit && push"
fi
