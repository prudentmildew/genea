#!/bin/sh
# Re-vendors i-slint-common at the pinned Slint version and re-applies
# Genea's font-scan patch (ADR 0004). Run it after every Slint pin bump.
#
#   scripts/reapply-slint-patch.sh           # uses the version in Cargo.toml
#   scripts/reapply-slint-patch.sh 1.19.0    # or an explicit one
#
# See patches/README.md for the whole procedure.
set -eu

repo=$(cd "$(dirname "$0")/.." && pwd)
cd "$repo"

version=${1:-$(sed -n 's/^slint = { version = "=\([0-9.]*\)".*/\1/p' Cargo.toml)}
[ -n "$version" ] || { echo "error: couldn't read the slint pin from Cargo.toml" >&2; exit 1; }
echo "i-slint-common $version"

# Make sure the crate is in the local registry, then find its sources.
cargo fetch --quiet
src=$(ls -d "${CARGO_HOME:-$HOME/.cargo}"/registry/src/*/"i-slint-common-$version" 2>/dev/null | head -n 1)
[ -n "$src" ] || { echo "error: i-slint-common-$version isn't in the cargo registry; run cargo fetch" >&2; exit 1; }

# Upstream may have added an opt-out by now; then drop the patch instead.
grep -n "system_fonts: true" "$src/sharedfontique.rs" >/dev/null || {
    echo "error: create_collection no longer has 'system_fonts: true'." >&2
    echo "Check whether upstream added an opt-out (ADR 0004: drop the patch then), or update the patch by hand." >&2
    exit 1
}

rm -rf patches/i-slint-common
cp -R "$src" patches/i-slint-common
chmod -R u+w patches/i-slint-common
patch --quiet -p1 -d patches/i-slint-common < patches/i-slint-common.patch

# Cargo silently builds the unpatched crate when the versions don't match
# ("Patch `i-slint-common` was not used in the crate graph"). Fail instead.
out=$(cargo tree -p genea-view -i i-slint-common --edges normal 2>&1) || { echo "$out" >&2; exit 1; }
if echo "$out" | grep -q "was not used in the crate graph"; then
    echo "$out" >&2
    echo "error: the i-slint-common patch isn't used; do the slint pin and the vendored version match?" >&2
    exit 1
fi
echo "$out" | head -n 1 | grep -q "patches/i-slint-common" || {
    echo "$out" >&2
    echo "error: i-slint-common doesn't resolve to patches/i-slint-common" >&2
    exit 1
}
echo "ok: i-slint-common $version patched; now run the benchmark harness (ADR 0004)."
