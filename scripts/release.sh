#!/bin/sh
# Builds a Genea release (ticket #64): an Apple Silicon Genea.app signed with
# Developer ID and the hardened runtime, notarized and stapled, inside a
# signed, notarized and stapled DMG. docs/releasing.md is the procedure.
#
#   scripts/release.sh           # the release; needs the credentials below
#   scripts/release.sh --local   # ad-hoc signed, not notarized: checks the
#                                # build, bundle and DMG without credentials
#
# Environment for a release:
#   GENEA_SIGNING_IDENTITY  e.g. "Developer ID Application: Jane Doe (TEAMID)"
#   GENEA_NOTARY_PROFILE    a notarytool keychain profile, made once with
#                           xcrun notarytool store-credentials
#
# Output: dist/Genea-<version>.dmg (and dist/Genea.app). It never publishes:
# the GitHub release is a separate, manual step (docs/releasing.md).
set -eu

repo=$(cd "$(dirname "$0")/.." && pwd)
cd "$repo"

local=false
case "${1:-}" in
    --local) local=true ;;
    "") ;;
    *) echo "usage: scripts/release.sh [--local]" >&2; exit 2 ;;
esac

# The oldest macOS the pinned Slint/winit/wgpu stack supports is older than
# 14, so 14 is the floor (spec #19). Keep in step with docs/releasing.md.
minimum_macos=14.0
target=aarch64-apple-darwin

die() { echo "error: $*" >&2; exit 1; }
step() { printf '\n==> %s\n' "$*"; }

version=$(sed -n '/^\[workspace.package\]/,/^\[/s/^version = "\(.*\)"/\1/p' Cargo.toml)
[ -n "$version" ] || die "couldn't read [workspace.package] version from Cargo.toml"
echo "Genea $version for $target, macOS $minimum_macos or later"

# --- Preflight -----------------------------------------------------------------

if $local; then
    identity=-
else
    identity=${GENEA_SIGNING_IDENTITY:?set GENEA_SIGNING_IDENTITY to your Developer ID Application identity}
    profile=${GENEA_NOTARY_PROFILE:?set GENEA_NOTARY_PROFILE to a notarytool keychain profile}
    [ -z "$(git status --porcelain)" ] || die "the working tree isn't clean; release from a committed state"
    # The update check compares the release's tag with the app's version.
    tag=$(git describe --exact-match --tags HEAD 2>/dev/null) || die "HEAD isn't tagged; tag it v$version first"
    [ "$tag" = "v$version" ] || die "HEAD is tagged $tag but Cargo.toml says $version"
    security find-identity -v -p codesigning | grep -F "\"$identity\"" >/dev/null ||
        die "no valid codesigning identity \"$identity\" in the keychain"
fi

step "Third-party licences"
if command -v "${CARGO_ABOUT:-cargo-about}" >/dev/null; then
    scripts/third-party-licences.sh --check
elif $local; then
    echo "warning: cargo-about isn't installed; not checking packaging/third-party-licences.txt" >&2
else
    die "cargo-about is needed to check the licences: cargo install cargo-about --locked --features cli --version 0.9.2"
fi

# --- Build ---------------------------------------------------------------------

step "Build"
MACOSX_DEPLOYMENT_TARGET=$minimum_macos cargo build --release --locked --target "$target" -p genea-view
binary=target/$target/release/genea

# Every Mach-O load command must allow the minimum macOS. A prebuilt static
# library built for a newer one would raise the real floor silently.
minos=$(vtool -show-build "$binary" | awk '/minos/ { print $2; exit }')
[ "$minos" = "$minimum_macos" ] || die "the binary's minos is $minos, not $minimum_macos"
lipo -archs "$binary" | grep -qx arm64 || die "the binary isn't arm64-only: $(lipo -archs "$binary")"

# --- Bundle --------------------------------------------------------------------

step "Bundle"
app=dist/Genea.app
rm -rf dist
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$binary" "$app/Contents/MacOS/genea"
sed -e "s/@VERSION@/$version/g" -e "s/@MINIMUM_MACOS@/$minimum_macos/g" packaging/Info.plist > "$app/Contents/Info.plist"
plutil -lint "$app/Contents/Info.plist" >/dev/null
cp packaging/third-party-licences.txt "$app/Contents/Resources/ThirdPartyLicences.txt"

# --- Sign ----------------------------------------------------------------------

step "Sign"
# The hardened runtime with no entitlements: Genea needs no JIT, no unsigned
# executable memory and no third-party dylibs. Child processes (Node, pnpm,
# Bun, tsgo) are separate executables with their own signatures.
if $local; then
    codesign --force --options runtime --sign - "$app"
else
    codesign --force --options runtime --timestamp --sign "$identity" "$app"
fi
codesign --verify --strict --verbose=2 "$app"
codesign --display --verbose=2 "$app" 2>&1 | grep -q 'flags=.*runtime' || die "the hardened runtime isn't on"

notarize() {
    # Waits for Apple's verdict; prints the log on failure.
    out=$(xcrun notarytool submit "$1" --keychain-profile "$profile" --wait 2>&1) || true
    echo "$out"
    echo "$out" | grep -q "status: Accepted" && return 0
    id=$(echo "$out" | awk '/^ *id:/ { print $2; exit }')
    [ -n "$id" ] && xcrun notarytool log "$id" --keychain-profile "$profile" >&2
    die "notarization of $1 failed"
}

if ! $local; then
    step "Notarize the app"
    ditto -c -k --keepParent "$app" dist/Genea.zip
    notarize dist/Genea.zip
    rm dist/Genea.zip
    # Stapled, so the app also opens offline once copied out of the DMG.
    xcrun stapler staple "$app"
    xcrun stapler validate "$app"
fi

# --- DMG -----------------------------------------------------------------------

step "DMG"
dmg=dist/Genea-$version.dmg
stage=$(mktemp -d -t genea-dmg)
trap 'rm -rf "$stage"' EXIT
ditto "$app" "$stage/Genea.app"
ln -s /Applications "$stage/Applications"
hdiutil create -quiet -volname "Genea $version" -srcfolder "$stage" -fs HFS+ -format UDZO -ov "$dmg"
if $local; then
    codesign --force --sign - "$dmg"
else
    codesign --force --timestamp --sign "$identity" "$dmg"
    step "Notarize the DMG"
    notarize "$dmg"
    xcrun stapler staple "$dmg"
    xcrun stapler validate "$dmg"
fi
hdiutil verify -quiet "$dmg"

# --- Verify --------------------------------------------------------------------

step "Verify"
# The app inside the DMG is the one that was signed.
mount=$(mktemp -d -t genea-mount)
hdiutil attach -quiet -nobrowse -readonly -mountpoint "$mount" "$dmg"
verified=true
codesign --verify --strict "$mount/Genea.app" || verified=false
hdiutil detach -quiet "$mount"
$verified || die "the app in the DMG doesn't verify"

if $local; then
    echo "Local build: ad-hoc signed and not notarized, so Gatekeeper would reject it."
else
    # What Gatekeeper decides on a clean machine for a downloaded DMG and app.
    spctl --assess --type open --context context:primary-signature --verbose=2 "$dmg"
    spctl --assess --type execute --verbose=2 "$app"
fi

step "Done"
shasum -a 256 "$dmg"
if ! $local; then
    echo "Publish it (manual): gh release create v$version $dmg --verify-tag --title \"Genea $version\" --notes-file <notes>"
fi
