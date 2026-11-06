#!/usr/bin/env bash
set -euo pipefail
OUT="$(cd "$(dirname "$0")/../build/jaspr" && pwd)"
rm -rf "$OUT/.dart_tool" "$OUT/packages" "$OUT/.build.manifest"
echo "cleaned deploy extras from $OUT"
