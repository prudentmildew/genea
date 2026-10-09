#!/usr/bin/env bash
# Re-resolves the Typical workspace's dependencies and rewrites the committed
# lockfile (typical/pnpm-lock.yaml). Run it after changing any dependency or
# manifest field in typical/lib/manifest.ts, then commit the new lockfile.
# Needs the network and pnpm on PATH (pnpm switches itself to the pinned version).
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
scratch="$(mktemp -d "${TMPDIR:-/tmp}/typical-lock.XXXXXX")"
trap 'rm -rf "$scratch"' EXIT

node "$here/generate.ts" "$scratch" >/dev/null
rm -f "$scratch/pnpm-lock.yaml"
(cd "$scratch" && pnpm install --lockfile-only)
cp "$scratch/pnpm-lock.yaml" "$here/pnpm-lock.yaml"
echo "Updated $here/pnpm-lock.yaml"
