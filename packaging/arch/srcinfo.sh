#!/usr/bin/env bash
# Regenerate .SRCINFO next to each PKGBUILD (needs makepkg, i.e. Arch).
set -euo pipefail
cd "$(dirname "$0")"
for d in mouser-git mouser-bin; do
  (cd "$d" && makepkg --printsrcinfo > .SRCINFO)
  echo "updated $d/.SRCINFO"
done
