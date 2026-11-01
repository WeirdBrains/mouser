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
# .SRCINFO without makepkg (CI runs on Ubuntu): same fields makepkg prints.
{
  echo "pkgbase = mouser-bin"
  sed -n "s/^pkgdesc=\"\(.*\)\"/\tpkgdesc = \1/p" PKGBUILD
  printf '\tpkgver = %s\n\tpkgrel = 1\n' "$ver"
  printf '\turl = %s\n\tarch = x86_64\n\tlicense = BSD-3-Clause\n' "$url"
  printf '\tmakedepends = ncurses\n'
  for d in glibc libgcc hicolor-icon-theme libxkbcommon wayland; do printf '\tdepends = %s\n' "$d"; done
  sed -n "s/^ *'\([^']*: [^']*\)'.*/\toptdepends = \1/p; s/^optdepends=('\([^']*\)'/\toptdepends = \1/p" PKGBUILD
  printf '\tprovides = mouser\n\tconflicts = mouser\n\tconflicts = mouser-git\n\toptions = !strip\n\toptions = !debug\n'
  printf '\tsource = mouser-src-%s.tar.gz::%s/archive/refs/tags/v%s.tar.gz\n' "$ver" "$url" "$ver"
  printf '\tsha256sums = %s\n' "$src_sum"
  printf '\tsource_x86_64 = mouser-%s-x86_64-unknown-linux-gnu.tar.gz::%s/releases/download/v%s/mouser-x86_64-unknown-linux-gnu.tar.gz\n' "$ver" "$url" "$ver"
  printf '\tsha256sums_x86_64 = %s\n' "$bin_sum"
  printf '\npkgname = mouser-bin\n'
} > .SRCINFO
echo "mouser-bin -> $ver (src $src_sum, bin $bin_sum)"
