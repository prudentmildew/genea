#!/usr/bin/env bash
# PROTOTYPE: fetches the spike's test files into fixtures/ (gitignored).
# checker.ts: TypeScript 5.9's type checker, a large real TS file (~54k lines).
# huge.ts: checker.ts repeated to ~100 MB, for the "open a 100 MB file" budget.
set -euo pipefail
cd "$(dirname "$0")"
mkdir -p fixtures
if [ ! -f fixtures/checker.ts ]; then
  curl -fsSL -o fixtures/checker.ts \
    https://raw.githubusercontent.com/microsoft/TypeScript/v5.9.3/src/compiler/checker.ts
fi
if [ ! -f fixtures/huge.ts ]; then
  : > fixtures/huge.ts
  while [ "$(stat -f%z fixtures/huge.ts)" -lt 100000000 ]; do
    cat fixtures/checker.ts >> fixtures/huge.ts
  done
fi
# typical.ts: a typical source file (first 1,500 lines of checker.ts).
head -n 1500 fixtures/checker.ts > fixtures/typical.ts
# onemb*.ts: two different ~1 MB slices, for the "open a file <= 1 MB" budget.
head -c 1000000 fixtures/checker.ts | sed '$d' > fixtures/onemb.ts
tail -c 1000000 fixtures/checker.ts | sed '1d' > fixtures/onemb-b.ts
wc -lc fixtures/*.ts
