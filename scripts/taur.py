#!/usr/bin/env python3
"""Publish the mouser-bin recipe to TAUR (the WeirdBrains AUR-compatible
server) with `taurctl pkg upload`. A required step of every release, after
the Release workflow's `packaging:` commit (docs/release.md §TAUR).

    taur.py srcinfo [PKGDIR]                  write PKGDIR/.SRCINFO from its PKGBUILD
    taur.py check VERSION [PKGDIR]            the recipe is VERSION's, checksums real, .SRCINFO matches
    taur.py pack VERSION [PKGDIR] [--out DIR] the upload archive (PKGBUILD + .SRCINFO in mouser-bin/)
    taur.py publish VERSION [PKGDIR] [--org ORG] [--dry-run] [--no-verify-release] [--taurctl PATH]

PKGDIR defaults to packaging/arch/mouser-bin; ORG to WeirdBrains.

`publish` runs `check`, downloads the release's source and binary tarballs and
compares them with the PKGBUILD's sha256sums (unless --no-verify-release),
builds the archive, checks `taurctl whoami` (signed in) and then runs
`taurctl pkg upload ARCHIVE --org ORG --yes`, then `taurctl info mouser-bin
--json` must report VERSION-PKGREL. --dry-run does every read-only step
(check, release download, archive, whoami) and prints the upload command
without running it. taurctl's own session (`taurctl login NAME`, a token
in its config dir) is the only credential; this script never reads it.

The .SRCINFO is generated from the PKGBUILD's literal assignments (the
fields makepkg --printsrcinfo prints, in its order), so it can't drift from
the PKGBUILD; with makepkg installed, `check` also compares with
`makepkg --printsrcinfo`. Python 3.8+, no dependencies.
"""
import argparse, hashlib, io, json, os, re, shutil, subprocess, sys, tarfile, tempfile, urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
PKGDIR = ROOT / "packaging" / "arch" / "mouser-bin"
ORG = "WeirdBrains"

# makepkg --printsrcinfo's order for the fields mouser-bin uses (and the common rest).
BASE_FIELDS = ["pkgdesc", "pkgver", "pkgrel", "epoch", "url", "install", "changelog", "arch", "groups", "license",
               "checkdepends", "makedepends", "depends", "optdepends", "provides", "conflicts", "replaces", "backup",
               "options", "source", "validpgpkeys", "noextract", "cksums", "md5sums", "sha1sums", "sha224sums",
               "sha256sums", "sha384sums", "sha512sums", "b2sums"]
ARCH_FIELDS = ["source", "checkdepends", "makedepends", "depends", "optdepends", "provides", "conflicts", "replaces",
               "cksums", "md5sums", "sha1sums", "sha224sums", "sha256sums", "sha384sums", "sha512sums", "b2sums"]
ARRAYS = {"arch", "groups", "license", "checkdepends", "makedepends", "depends", "optdepends", "provides", "conflicts",
          "replaces", "backup", "options", "source", "validpgpkeys", "noextract", "cksums", "md5sums", "sha1sums",
          "sha224sums", "sha256sums", "sha384sums", "sha512sums", "b2sums"}


class TaurError(Exception):
    pass


def _words(s):
    """Shell words of an array body: '...' and "..." quoted or bare."""
    out, i = [], 0
    while i < len(s):
        c = s[i]
        if c.isspace():
            i += 1
        elif c == "#":
            j = s.find("\n", i)
            i = len(s) if j < 0 else j
        elif c in "'\"":
            j = s.find(c, i + 1)
            if j < 0:
                raise TaurError(f"unterminated quote in {s[i:i + 40]!r}")
            out.append((s[i + 1:j], c == '"'))
            i = j + 1
        else:
            j = i
            while j < len(s) and not s[j].isspace():
                j += 1
            out.append((s[i:j], True))
            i = j
    return out


def parse_pkgbuild(text):
    """The top-level literal assignments (`name=value`, `name=(...)`), with
    ${var} / $var expanded from earlier ones. Functions are skipped."""
    vals, i, lines = {}, 0, text.splitlines()

    def expand(v, dq):
        if not dq:
            return v
        return re.sub(r"\$\{(\w+)\}|\$(\w+)", lambda m: str(vals.get(m.group(1) or m.group(2), "")) if not isinstance(
            vals.get(m.group(1) or m.group(2)), list) else " ".join(vals[m.group(1) or m.group(2)]), v)

    while i < len(lines):
        line = lines[i]
        m = re.match(r"^([A-Za-z_][A-Za-z0-9_]*)=(.*)$", line)
        if not m:
            i += 1
            continue
        name, rest = m.groups()
        if rest.startswith("("):
            body = rest[1:]
            while ")" not in _strip_quoted(body):
                i += 1
                if i >= len(lines):
                    raise TaurError(f"unterminated array {name}")
                body += "\n" + lines[i]
            body = body[: _close_paren(body)]
            vals[name] = [expand(w, dq) for w, dq in _words(body)]
        else:
            ws = _words(rest)
            vals[name] = expand(ws[0][0], ws[0][1]) if ws else ""
        i += 1
    return vals


def _strip_quoted(s):
    return re.sub(r"'[^']*'|\"[^\"]*\"", "", s)


def _close_paren(s):
    q = None
    for k, c in enumerate(s):
        if q:
            if c == q:
                q = None
        elif c in "'\"":
            q = c
        elif c == ")":
            return k
    raise TaurError("unterminated array")


def srcinfo(text):
    """.SRCINFO for a single-package PKGBUILD (what makepkg --printsrcinfo prints)."""
    v = parse_pkgbuild(text)
    name = v.get("pkgname")
    if not isinstance(name, str) or not name:
        raise TaurError("PKGBUILD has no single pkgname")
    out = [f"pkgbase = {v.get('pkgbase', name)}"]
    for f in BASE_FIELDS:
        x = v.get(f)
        if x is None or x == "" or x == []:
            continue
        for item in (x if isinstance(x, list) else [x]):
            out.append(f"\t{f} = {item}")
    arches = v.get("arch", [])
    for a in arches if isinstance(arches, list) else [arches]:
        for f in ARCH_FIELDS:
            for item in v.get(f"{f}_{a}", []) or []:
                out.append(f"\t{f}_{a} = {item}")
    out += ["", f"pkgname = {name}", ""]
    return "\n".join(out)


HEX64 = re.compile(r"^[0-9a-f]{64}$")


def check(version, pkgdir, makepkg=None):
    """Problems with the recipe for VERSION (empty when it can be published)."""
    pkgdir = Path(pkgdir)
    errs = []
    try:
        text = (pkgdir / "PKGBUILD").read_text()
        have = (pkgdir / ".SRCINFO").read_text()
    except OSError as e:
        return [f"missing recipe file: {e}"]
    v = parse_pkgbuild(text)
    if v.get("pkgver") != version:
        errs.append(f"PKGBUILD pkgver is {v.get('pkgver')!r}, not {version!r} (run packaging/arch/update-bin.sh {version})")
    if not re.fullmatch(r"\d+", str(v.get("pkgrel", ""))):
        errs.append(f"PKGBUILD pkgrel {v.get('pkgrel')!r} is not a number")
    for f in ("sha256sums", "sha256sums_x86_64"):
        sums = v.get(f) or []
        if not sums or not all(HEX64.match(s) for s in sums):
            errs.append(f"{f} {sums!r} is not a real sha256 (SKIP and placeholders can't be published)")
    for s in (v.get("source") or []) + (v.get("source_x86_64") or []):
        if f"v{version}" not in s:
            errs.append(f"source {s!r} is not v{version}'s")
    m = re.search(r"^# Maintainer:\s*(.*)$", text, re.M)
    if not m or "placeholder" in m.group(1).lower() or not m.group(1).strip():
        errs.append("the # Maintainer: line is missing or a placeholder")
    want = srcinfo(text)
    if have != want:
        errs.append(".SRCINFO doesn't match the PKGBUILD (run scripts/taur.py srcinfo)")
    if makepkg and shutil.which(makepkg):
        r = subprocess.run([makepkg, "--printsrcinfo"], cwd=pkgdir, capture_output=True, text=True)
        if r.returncode != 0:
            errs.append(f"makepkg --printsrcinfo failed: {r.stderr.strip()[:300]}")
        elif r.stdout != have:
            errs.append(".SRCINFO differs from makepkg --printsrcinfo")
    return errs


def pack(pkgdir, out_dir, version):
    """mouser-bin-VERSION-taur.tar.gz: mouser-bin/PKGBUILD and .SRCINFO only,
    reproducible (fixed mtime and owner). TAUR takes one folder, no nesting."""
    pkgdir = Path(pkgdir)
    base = parse_pkgbuild((pkgdir / "PKGBUILD").read_text())["pkgname"]
    out = Path(out_dir) / f"{base}-{version}-taur.tar.gz"
    buf = io.BytesIO()
    with tarfile.open(fileobj=buf, mode="w", format=tarfile.PAX_FORMAT) as t:
        d = tarfile.TarInfo(base)
        d.type, d.mode, d.mtime = tarfile.DIRTYPE, 0o755, 0
        t.addfile(d)
        for n in ("PKGBUILD", ".SRCINFO"):
            data = (pkgdir / n).read_bytes()
            if len(data) > 256 * 1024:
                raise TaurError(f"{n} is over TAUR's 256 KiB file limit")
            i = tarfile.TarInfo(f"{base}/{n}")
            i.size, i.mode, i.mtime = len(data), 0o644, 0
            t.addfile(i, io.BytesIO(data))
    import gzip
    out.write_bytes(gzip.compress(buf.getvalue(), mtime=0))
    return out


def verify_release(pkgdir, fetch=None):
    """Download every source in the PKGBUILD and compare with its sha256."""
    v = parse_pkgbuild((Path(pkgdir) / "PKGBUILD").read_text())
    fetch = fetch or _download_sha256
    errs = []
    for sf, cf in (("source", "sha256sums"), ("source_x86_64", "sha256sums_x86_64")):
        for s, want in zip(v.get(sf) or [], v.get(cf) or []):
            url = s.split("::", 1)[-1]
            try:
                got = fetch(url)
            except Exception as e:
                errs.append(f"can't download {url}: {e}")
                continue
            if got != want:
                errs.append(f"{url}: sha256 {got} but the PKGBUILD says {want}")
    return errs


def _download_sha256(url):
    h = hashlib.sha256()
    with urllib.request.urlopen(url, timeout=120) as r:
        for chunk in iter(lambda: r.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def run(cmd, **kw):
    return subprocess.run(cmd, capture_output=True, text=True, **kw)


def publish(version, pkgdir, org, dry_run, verify, taurctl, out=sys.stdout, fetch=None):
    errs = check(version, pkgdir, makepkg="makepkg")
    if errs:
        raise TaurError("recipe not ready:\n  " + "\n  ".join(errs))
    if verify:
        errs = verify_release(pkgdir, fetch)
        if errs:
            raise TaurError("release doesn't match the PKGBUILD:\n  " + "\n  ".join(errs))
        print("release tarballs match the PKGBUILD's sha256sums", file=out)
    if not shutil.which(taurctl):
        raise TaurError(f"{taurctl} not found: install taurctl (the taur package) first")
    tmp = Path(tempfile.mkdtemp(prefix="mouser-taur-"))
    try:
        archive = pack(pkgdir, tmp, version)
        print(f"archive {archive.name} ({archive.stat().st_size} bytes): mouser-bin/PKGBUILD, mouser-bin/.SRCINFO", file=out)
        who = run([taurctl, "whoami"])
        if who.returncode != 0:
            raise TaurError(f"taurctl is not signed in ({who.stderr.strip() or who.stdout.strip()}); run: taurctl login NAME")
        print(f"taurctl: {who.stdout.strip()}", file=out)
        cmd = [taurctl, "pkg", "upload", str(archive), "--org", org, "--yes"]
        if dry_run:
            print("[dry-run] " + " ".join(cmd[:3] + [archive.name] + cmd[4:]), file=out)
            return 0
        up = run(cmd)
        if up.returncode != 0:
            raise TaurError(f"taurctl pkg upload failed: {(up.stderr or up.stdout).strip()}")
        print(up.stdout.strip(), file=out)
        v = parse_pkgbuild((Path(pkgdir) / "PKGBUILD").read_text())
        name, want = v["pkgname"], f"{version}-{v['pkgrel']}"
        info = run([taurctl, "info", name, "--json"])
        try:
            res = json.loads(info.stdout)["results"]
            got = res[0]["Version"] if res else None
        except (ValueError, KeyError, IndexError, TypeError):
            got = None
        if got != want:
            raise TaurError(f"uploaded, but TAUR reports {name} {got!r}, not {want!r}")
        print(f"TAUR has {name} {want}", file=out)
        return 0
    finally:
        shutil.rmtree(tmp, ignore_errors=True)


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    sub = ap.add_subparsers(dest="cmd", required=True)
    s = sub.add_parser("srcinfo", help="write PKGDIR/.SRCINFO from its PKGBUILD")
    s.add_argument("pkgdir", nargs="?", default=str(PKGDIR))
    c = sub.add_parser("check", help="the recipe is ready to publish for VERSION")
    c.add_argument("version")
    c.add_argument("pkgdir", nargs="?", default=str(PKGDIR))
    p = sub.add_parser("pack", help="write the upload archive")
    p.add_argument("version")
    p.add_argument("pkgdir", nargs="?", default=str(PKGDIR))
    p.add_argument("--out", default=".")
    u = sub.add_parser("publish", help="check, verify the release, pack and taurctl pkg upload")
    u.add_argument("version")
    u.add_argument("pkgdir", nargs="?", default=str(PKGDIR))
    u.add_argument("--org", default=ORG)
    u.add_argument("--dry-run", action="store_true")
    u.add_argument("--no-verify-release", action="store_true")
    u.add_argument("--taurctl", default=os.environ.get("TAURCTL", "taurctl"))
    a = ap.parse_args(argv)
    try:
        version = getattr(a, "version", "").lstrip("v")
        if a.cmd == "srcinfo":
            d = Path(a.pkgdir)
            (d / ".SRCINFO").write_text(srcinfo((d / "PKGBUILD").read_text()))
            print(f"wrote {d / '.SRCINFO'}")
        elif a.cmd == "check":
            errs = check(version, a.pkgdir, makepkg="makepkg")
            for e in errs:
                print(f"taur: {e}", file=sys.stderr)
            if errs:
                return 1
            print(f"{Path(a.pkgdir).name} {version}: ready for TAUR")
        elif a.cmd == "pack":
            errs = check(version, a.pkgdir)
            if errs:
                raise TaurError("recipe not ready:\n  " + "\n  ".join(errs))
            print(pack(a.pkgdir, a.out, version))
        else:
            return publish(version, a.pkgdir, a.org, a.dry_run, not a.no_verify_release, a.taurctl)
    except TaurError as e:
        print(f"taur: {e}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
