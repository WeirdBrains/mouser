#!/usr/bin/env bash
# Point mouser-bin/PKGBUILD at release VERSION (e.g. 0.7.3) and fill in real
# sha256sums from the published GitHub release. Run after the release exists.
# Usage: packaging/arch/update-bin.sh 0.7.3
set -euo pipefail
ver="${1#v}"
cd "$(dirname "$0")/mouser-bin"
url="https://github.com/WeirdBrains/mouser"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
curl -fsSL -o "$tmp/src.tar.gz" "$url/archive/refs/tags/v$ver.tar.gz"
curl -fsSL -o "$tmp/bin.tar.gz" "$url/releases/download/v$ver/mouser-x86_64-unknown-linux-gnu.tar.gz"
src_sum="$(sha256sum "$tmp/src.tar.gz" | cut -d' ' -f1)"
bin_sum="$(sha256sum "$tmp/bin.tar.gz" | cut -d' ' -f1)"
sed -i -e "s/^pkgver=.*/pkgver=$ver/" -e "s/^pkgrel=.*/pkgrel=1/" \
  -e "s/^sha256sums=.*/sha256sums=('$src_sum')/" \
  -e "s/^sha256sums_x86_64=.*/sha256sums_x86_64=('$bin_sum')/" PKGBUILD
# .SRCINFO from the PKGBUILD itself (CI runs on Ubuntu, no makepkg); the same
# text makepkg --printsrcinfo prints (scripts/test-taur.py checks that on Arch).
python3 ../../../scripts/taur.py srcinfo .
echo "mouser-bin -> $ver (src $src_sum, bin $bin_sum)"
