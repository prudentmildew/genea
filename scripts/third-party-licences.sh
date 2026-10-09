#!/bin/sh
# Regenerates packaging/third-party-licences.txt, the list of third-party
# software and licences that the About dialog shows (ticket #64). Run it
# after any dependency change; scripts/release.sh refuses to build a release
# while the file is stale.
#
#   scripts/third-party-licences.sh           # rewrite the file
#   scripts/third-party-licences.sh --check   # fail if it is out of date
#
# Needs cargo-about 0.9.2 (cargo install cargo-about --locked --features cli
# --version 0.9.2), on PATH or in $CARGO_ABOUT. Runs offline.
set -eu

repo=$(cd "$(dirname "$0")/.." && pwd)
cd "$repo"

about=${CARGO_ABOUT:-cargo-about}
command -v "$about" >/dev/null || {
    echo "error: cargo-about isn't installed: cargo install cargo-about --locked --features cli --version 0.9.2" >&2
    exit 1
}

out=packaging/third-party-licences.txt
tmp=$(mktemp -t genea-licences)
trap 'rm -f "$tmp"' EXIT

# Every crate in the app (genea-view's graph on aarch64-apple-darwin,
# without build and dev dependencies) and the texts cargo-about finds.
"$about" -L error generate --frozen --fail \
    -c packaging/about.toml -m crates/genea-view/Cargo.toml \
    -o "$tmp" packaging/licences/third-party.hbs

section() {
    printf '\n================================================================================\n'
    printf '%s\n' "$1"
    printf '================================================================================\n\n'
}

{
    # Slint ships its royalty-free licence as a LicenseRef, whose text
    # cargo-about can't pick up. It's the same file in every Slint crate.
    section "Slint: Slint Royalty-free Desktop, Mobile, and Web Applications License 2.0"
    printf 'Used by: the slint and i-slint-* crates listed above.\n\n'
    cat patches/i-slint-common/LICENSES/LicenseRef-Slint-Royalty-free-2.0.md

    # Not crates.
    section "Inter font (SIL Open Font License 1.1)"
    printf 'Embedded by i-slint-common as its fallback font.\n\n'
    cat packaging/licences/Inter.txt

    section "Skia (BSD-3-Clause)"
    printf 'The 2D graphics library, linked from the prebuilt binaries of skia-bindings.\n\n'
    cat packaging/licences/Skia.txt
} >> "$tmp"

if [ "${1:-}" = "--check" ]; then
    if ! cmp -s "$tmp" "$out"; then
        echo "error: $out is out of date; run scripts/third-party-licences.sh and commit it" >&2
        exit 1
    fi
    echo "$out is up to date"
else
    cp "$tmp" "$out"
    echo "wrote $out ($(grep -c '' "$out") lines)"
fi
