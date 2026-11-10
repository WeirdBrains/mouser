#!/usr/bin/env python3
"""Homebrew formula and cask for a Mouser release, with real checksums.

    packaging.py brew VERSION --assets DIR   # files from the Release workflow
    packaging.py brew VERSION --release      # download the published assets
    packaging.py check [ROOT]                # the committed files are sound

`brew` hashes the macOS tarballs (formula) and dmgs (cask) for both
architectures, checks each against its published `.sha256` when there is one,
and writes Formula/mouser.rb and Casks/mouser.rb. It refuses (exit 1, nothing
written) when an asset is missing, found twice with different contents, or
doesn't match its `.sha256`, so a release never commits an empty hash.
`check` fails on an empty or malformed sha256, a url for another version than
`version`, or a missing architecture. Python 3.8+, no dependencies.
docs/release.md §Homebrew.
"""
import argparse, hashlib, os, re, sys, tempfile, urllib.request
from pathlib import Path

REPO = "https://github.com/WeirdBrains/mouser"
ARCHS = {"arm": "aarch64-apple-darwin", "intel": "x86_64-apple-darwin"}
FORMULA_ASSET = "mouser-{triple}.tar.gz"
CASK_ASSET = "Mouser-{triple}.dmg"
HEX = re.compile(r"^[0-9a-f]{64}$")
VERSION = re.compile(r"^\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?$")

FORMULA = """class Mouser < Formula
  desc "Mouser terminal emulator"
  homepage "https://github.com/WeirdBrains/mouser"
  version "{ver}"
  on_macos do
    on_arm do
      url "{url_arm}"
      sha256 "{sha_arm}"
    end
    on_intel do
      url "{url_intel}"
      sha256 "{sha_intel}"
    end
  end
  def install
    bin.install "mouser-app" => "mouser"
    (lib/"mouser").install "engines"
  end
end
"""

CASK = """cask "mouser" do
  version "{ver}"
  on_arm do
    sha256 "{sha_arm}"
    url "{url_arm}"
  end
  on_intel do
    sha256 "{sha_intel}"
    url "{url_intel}"
  end
  name "Mouser"
  desc "Mouser terminal emulator"
  homepage "https://github.com/WeirdBrains/mouser"
  auto_updates false
  app "Mouser.app"
end
"""


class PackagingError(Exception):
    pass


def url_for(ver, name):
    return f"{REPO}/releases/download/v{ver}/{name}"


def sha256_file(p):
    h = hashlib.sha256()
    with open(p, "rb") as f:
        for b in iter(lambda: f.read(1 << 20), b""):
            h.update(b)
    return h.hexdigest()


def sidecar(text, name):
    """The hash in a `.sha256` file (`HASH` or `HASH  name`)."""
    parts = text.split()
    if not parts or not HEX.match(parts[0].lower()):
        raise PackagingError(f"{name}.sha256 holds no sha256: {text[:80]!r}")
    return parts[0].lower()


def from_dir(root, name):
    """Hash of asset `name` under `root` (searched recursively: the workflow
    downloads each artifact into its own folder)."""
    found = sorted(Path(root).rglob(name))
    found = [p for p in found if p.is_file()]
    if not found:
        raise PackagingError(f"{name} is not in {root}")
    sums = {sha256_file(p) for p in found}
    if len(sums) > 1:
        raise PackagingError(f"{name} is in {root} {len(found)} times with different contents")
    got = sums.pop()
    for p in found:
        sc = p.with_name(p.name + ".sha256")
        if sc.is_file() and sidecar(sc.read_text(), name) != got:
            raise PackagingError(f"{name} does not match its .sha256 ({p})")
    return got


def fetch(url, dest):
    req = urllib.request.Request(url, headers={"User-Agent": "mouser-packaging"})
    with urllib.request.urlopen(req, timeout=120) as r, open(dest, "wb") as f:
        while True:
            b = r.read(1 << 20)
            if not b:
                break
            f.write(b)


def from_release(ver, name, tmp):
    """Download the published asset and its .sha256; they must agree."""
    dest = Path(tmp) / name
    try:
        fetch(url_for(ver, name), dest)
        fetch(url_for(ver, name) + ".sha256", Path(tmp) / (name + ".sha256"))
    except OSError as e:
        raise PackagingError(f"could not download {name} for v{ver}: {e}") from e
    got = sha256_file(dest)
    if sidecar((Path(tmp) / (name + ".sha256")).read_text(), name) != got:
        raise PackagingError(f"{name}: the download does not match the published .sha256")
    dest.unlink()
    return got


def render(ver, sums):
    """Formula and cask text from {(kind, arch): sha}; every value checked."""
    if not VERSION.match(ver):
        raise PackagingError(f"not a version: {ver!r}")
    for k, v in sums.items():
        if not HEX.match(v or ""):
            raise PackagingError(f"{k}: not a sha256: {v!r}")
    f = FORMULA.format(
        ver=ver,
        url_arm=url_for(ver, FORMULA_ASSET.format(triple=ARCHS["arm"])),
        sha_arm=sums[("formula", "arm")],
        url_intel=url_for(ver, FORMULA_ASSET.format(triple=ARCHS["intel"])),
        sha_intel=sums[("formula", "intel")],
    )
    c = CASK.format(
        ver=ver,
        url_arm=url_for(ver, CASK_ASSET.format(triple=ARCHS["arm"])),
        sha_arm=sums[("cask", "arm")],
        url_intel=url_for(ver, CASK_ASSET.format(triple=ARCHS["intel"])),
        sha_intel=sums[("cask", "intel")],
    )
    return f, c


def assets():
    for kind, pat in (("formula", FORMULA_ASSET), ("cask", CASK_ASSET)):
        for arch, triple in ARCHS.items():
            yield (kind, arch), pat.format(triple=triple)


def brew(ver, root, asset_dir=None, release=False):
    ver = ver.lstrip("v")
    sums = {}
    with tempfile.TemporaryDirectory() as tmp:
        for key, name in assets():
            sums[key] = from_release(ver, name, tmp) if release else from_dir(asset_dir, name)
    f, c = render(ver, sums)
    for text, kind in ((f, "formula"), (c, "cask")):
        problems = check_text(text, kind)
        if problems:
            raise PackagingError("; ".join(problems))
    (root / "Formula").mkdir(exist_ok=True)
    (root / "Casks").mkdir(exist_ok=True)
    (root / "Formula/mouser.rb").write_text(f)
    (root / "Casks/mouser.rb").write_text(c)
    return sums


def check_text(text, kind):
    """Problems in a formula or cask's text (empty: sound)."""
    out = []
    m = re.search(r'^\s*version "([^"]*)"', text, re.M)
    if not m or not VERSION.match(m.group(1)):
        return [f"{kind}: no version"]
    ver = m.group(1)
    for arch in ARCHS:
        block = re.search(rf"on_{arch} do\n(.*?)\n\s*end", text, re.S)
        if not block:
            out.append(f"{kind}: no on_{arch} block")
            continue
        b = block.group(1)
        sha = re.search(r'sha256 "([^"]*)"', b)
        url = re.search(r'url "([^"]*)"', b)
        if not sha or not HEX.match(sha.group(1)):
            out.append(f"{kind} {arch}: sha256 is {sha.group(1) if sha else 'missing'!r}, not a sha256")
        pat = FORMULA_ASSET if kind == "formula" else CASK_ASSET
        want = url_for(ver, pat.format(triple=ARCHS[arch]))
        if not url or url.group(1) != want:
            out.append(f"{kind} {arch}: url {url.group(1) if url else None!r}, want {want}")
    return out


def check(root):
    problems = []
    for rel, kind in (("Formula/mouser.rb", "formula"), ("Casks/mouser.rb", "cask")):
        p = root / rel
        if not p.is_file():
            problems.append(f"{rel} is missing")
            continue
        problems += check_text(p.read_text(), kind)
    return problems


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    sub = ap.add_subparsers(dest="cmd", required=True)
    b = sub.add_parser("brew", help="write Formula/mouser.rb and Casks/mouser.rb for VERSION")
    b.add_argument("version")
    src = b.add_mutually_exclusive_group(required=True)
    src.add_argument("--assets", help="folder with the release files (searched recursively)")
    src.add_argument("--release", action="store_true", help="download the published assets")
    b.add_argument("--root", default=Path(__file__).resolve().parent.parent, type=Path)
    c = sub.add_parser("check", help="fail on an empty or wrong hash or url")
    c.add_argument("root", nargs="?", default=Path(__file__).resolve().parent.parent, type=Path)
    a = ap.parse_args(argv)
    try:
        if a.cmd == "brew":
            sums = brew(a.version, a.root, a.assets, a.release)
            for (kind, arch), s in sorted(sums.items()):
                print(f"{kind} {arch}: {s}")
        else:
            problems = check(a.root)
            if problems:
                raise PackagingError("\n".join(problems))
            print("Formula/mouser.rb and Casks/mouser.rb: every architecture has its url and sha256")
    except PackagingError as e:
        print(f"packaging: {e}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
