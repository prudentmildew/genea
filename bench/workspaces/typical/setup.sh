#!/usr/bin/env bash
# Builds the Typical reference workspace from scratch:
#   1. generates it into OUT_DIR (default bench/workspaces/out/typical),
#   2. installs node_modules from the committed lockfile (--frozen-lockfile),
#   3. reports its figures against the targets (fails if any is off by > 10 %),
#   4. with --typecheck, type-checks it with the workspace's own TS 7 (tsgo):
#      the root program (`tsc -b --noEmit`, the project check) and each
#      package's tsconfig (what the language server opens).
#
# Usage: typical/setup.sh [--typecheck] [OUT_DIR]
# Needs Node >= 24 and pnpm on PATH (pnpm switches itself to the version pinned
# in packageManager). The first install needs the network.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
typecheck=0
out=""
for arg in "$@"; do
  case "$arg" in
    --typecheck) typecheck=1 ;;
    -*) echo "unknown option: $arg" >&2; exit 2 ;;
    *) out="$arg" ;;
  esac
done
out="${out:-$here/../out/typical}"

rm -rf "$out"
node "$here/generate.ts" "$out"

# Make it a git repository with one commit at a fixed date, so the commit id
# fingerprints the generated tree, git features have a baseline, and the
# ignore rules of any enclosing repository (this one) don't hide its files
# from tools that honour .gitignore.
(
  cd "$out"
  export GIT_AUTHOR_NAME="Genea bench" GIT_AUTHOR_EMAIL="bench@genea.invalid"
  export GIT_COMMITTER_NAME="Genea bench" GIT_COMMITTER_EMAIL="bench@genea.invalid"
  export GIT_AUTHOR_DATE="2026-01-01T00:00:00Z" GIT_COMMITTER_DATE="2026-01-01T00:00:00Z"
  export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1
  git init -q -b main
  git add -A
  git -c commit.gpgsign=false -c core.hooksPath=/dev/null commit -q -m "Typical reference workspace"
  echo "Typical workspace commit: $(git rev-parse HEAD)"
)

(cd "$out" && pnpm install --frozen-lockfile --reporter=append-only)
# pnpm must not rewrite generated files (pnpm 11 edits pnpm-workspace.yaml
# when, say, a dependency's build script isn't listed in allowBuilds).
if [[ -n "$(git -C "$out" status --porcelain)" ]]; then
  git -C "$out" status --short >&2
  echo "pnpm install changed generated files in $out" >&2
  exit 1
fi

node "$here/report.ts" "$out"

if [[ "$typecheck" == 1 ]]; then
  tsc="$out/node_modules/.bin/tsc"
  echo "Type-checking with $("$tsc" --version)"
  (cd "$out" && "$tsc" -b --noEmit)
  for pkg in "$out"/packages/*/; do
    (cd "$pkg" && "$tsc" --noEmit -p .)
  done
  rm -f "$out"/*.tsbuildinfo
  echo "No type errors."
fi
