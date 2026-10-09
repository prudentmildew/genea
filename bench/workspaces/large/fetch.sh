#!/usr/bin/env bash
# Fetches the Large reference workspace, microsoft/vscode at a pinned commit,
# and installs its dependencies.
#
# Usage: large/fetch.sh [--ignore-scripts | --no-install] [OUT_DIR]
#   OUT_DIR           default bench/workspaces/out/large
#   (no option)       `npm ci` at the root, as vscode documents it. Its
#                     pre/postinstall scripts install every sub-directory and
#                     build native modules, so it needs Node at the version in
#                     vscode's .nvmrc (24.18.0 at the pinned commit), npm < 13
#                     and the Xcode command line tools.
#   --ignore-scripts  `npm ci --ignore-scripts` in every directory vscode's
#                     postinstall would install (build/npm/dirs.ts). Same
#                     node_modules trees minus native builds; no compiler needed.
#   --no-install      fetch only.
#
# An existing vscode checkout in OUT_DIR is moved to the pinned commit and
# cleaned (`git clean -ffdx`, which also removes node_modules).
set -euo pipefail

VSCODE_REPO="https://github.com/microsoft/vscode.git"
VSCODE_COMMIT="2a59476c9bfcb90b3ddc372c36762471b7dfad1c" # tag 1.141.0, 2026-10-06

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
install="full"
out=""
for arg in "$@"; do
  case "$arg" in
    --ignore-scripts) install="ignore-scripts" ;;
    --no-install) install="none" ;;
    -*) echo "unknown option: $arg" >&2; exit 2 ;;
    *) out="$arg" ;;
  esac
done
out="${out:-$(cd "$here/.." && pwd)/out/large}"

if [[ -e "$out" && ! -d "$out/.git" ]]; then
  echo "$out exists and is not a git checkout; refusing to touch it" >&2
  exit 1
fi

if [[ ! -d "$out/.git" ]]; then
  mkdir -p "$out"
  git -C "$out" init -q
  git -C "$out" remote add origin "$VSCODE_REPO"
fi
if [[ "$(git -C "$out" rev-parse -q --verify HEAD || true)" != "$VSCODE_COMMIT" ]]; then
  echo "Fetching vscode at $VSCODE_COMMIT"
  git -C "$out" fetch -q --depth 1 origin "$VSCODE_COMMIT"
  git -C "$out" -c advice.detachedHead=false checkout -q --force "$VSCODE_COMMIT"
  git -C "$out" clean -q -ffdx
fi
echo "vscode at $(git -C "$out" rev-parse HEAD) in $out"

case "$install" in
  full)
    want="$(cat "$out/.nvmrc")"
    have="$(node --version)"
    if [[ "v$want" != "$have" ]]; then
      echo "warning: vscode pins Node $want (.nvmrc), running $have" >&2
    fi
    (cd "$out" && npm ci)
    ;;
  ignore-scripts)
    dirs="$(cd "$out" && node --input-type=module -e \
      'const { dirs } = await import("./build/npm/dirs.ts"); console.log(dirs.join("\n"));')"
    while IFS= read -r dir; do
      if [[ -f "$out/$dir/package-lock.json" ]]; then
        echo "npm ci --ignore-scripts in ${dir:-.}"
        (cd "$out/$dir" && npm ci --ignore-scripts --no-audit --no-fund --loglevel=error)
      fi
    done <<< "$dirs"
    ;;
  none) ;;
esac

node "$here/report.ts" "$out"
