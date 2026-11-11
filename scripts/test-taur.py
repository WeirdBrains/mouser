#!/usr/bin/env python3
"""Tests for scripts/taur.py (no network, no real TAUR: a fake taurctl).

    python3 scripts/test-taur.py [-v]
"""
import contextlib, hashlib, importlib.util, io, json, os, shutil, stat, subprocess, sys, tarfile, tempfile, unittest
from pathlib import Path

sys.dont_write_bytecode = True
HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("taur", HERE / "taur.py")
tr = importlib.util.module_from_spec(spec)
spec.loader.exec_module(tr)
REAL = HERE.parent / "packaging" / "arch" / "mouser-bin"

SRC, BIN = b"source tarball", b"binary tarball"


def recipe(d, ver="2.0.0", maint="Michael Christenson II <m3talsmith@gmail.com>", src=None, binsum=None):
    """The real PKGBUILD pointed at VER with the fake tarballs' sums."""
    text = (REAL / "PKGBUILD").read_text()
    lines = []
    for l in text.splitlines():
        if l.startswith("# Maintainer:"):
            l = f"# Maintainer: {maint}"
        elif l.startswith("pkgver="):
            l = f"pkgver={ver}"
        elif l.startswith("sha256sums="):
            l = f"sha256sums=('{src or hashlib.sha256(SRC).hexdigest()}')"
        elif l.startswith("sha256sums_x86_64="):
            l = f"sha256sums_x86_64=('{binsum or hashlib.sha256(BIN).hexdigest()}')"
        lines.append(l)
    d.mkdir(parents=True, exist_ok=True)
    (d / "PKGBUILD").write_text("\n".join(lines) + "\n")
    (d / ".SRCINFO").write_text(tr.srcinfo((d / "PKGBUILD").read_text()))
    return d


def fake_fetch(url):
    return hashlib.sha256(BIN if "releases/download" in url else SRC).hexdigest()


FAKE = r'''#!/bin/sh
echo "$@" >> "$TAUR_FAKE_LOG"
case "$1" in
  whoami) [ -n "$TAUR_FAKE_OUT" ] && { echo "not signed in" >&2; exit 1; }; echo "m3talsmith <m@example.com> (verified)";;
  pkg) [ -n "$TAUR_FAKE_UPLOAD_FAIL" ] && { echo "rejected: .SRCINFO mismatch" >&2; exit 1; }
       tar tzf "$3" > "$TAUR_FAKE_LOG.tar"; echo "uploaded mouser-bin";;
  info) echo "{\"resultcount\":1,\"results\":[{\"Name\":\"mouser-bin\",\"Version\":\"${TAUR_FAKE_VERSION:-2.0.0-1}\"}]}";;
esac
'''


class Srcinfo(unittest.TestCase):
    def test_committed_srcinfo_is_generated_from_the_pkgbuild(self):
        self.assertEqual((REAL / ".SRCINFO").read_text(), tr.srcinfo((REAL / "PKGBUILD").read_text()))

    @unittest.skipUnless(shutil.which("makepkg"), "makepkg (Arch) not installed")
    def test_same_as_makepkg(self):
        for d in (REAL,):
            r = subprocess.run(["makepkg", "--printsrcinfo"], cwd=d, capture_output=True, text=True, check=True)
            self.assertEqual(r.stdout, tr.srcinfo((d / "PKGBUILD").read_text()), d)

    def test_every_pkgbuild_dependency_is_listed(self):
        si = tr.srcinfo((REAL / "PKGBUILD").read_text())
        for dep in tr.parse_pkgbuild((REAL / "PKGBUILD").read_text())["depends"]:
            self.assertIn(f"\tdepends = {dep}\n", si)

    def test_parsing(self):
        v = tr.parse_pkgbuild("pkgname=x\n_n=y\npkgver=1.2\nsource=(\"${_n}-$pkgver.tgz::u/v$pkgver\" 'lit$x' # c\n  b)\nf() {\n  a=1\n}\n")
        self.assertEqual(v["source"], ["y-1.2.tgz::u/v1.2", "lit$x", "b"])
        self.assertNotIn("a", v)


class Check(unittest.TestCase):
    def setUp(self):
        self.t = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.t)

    def test_ready(self):
        self.assertEqual(tr.check("2.0.0", recipe(self.t / "r")), [])

    def test_refusals(self):
        cases = {
            "pkgver": recipe(self.t / "a", ver="1.9.0"),
            "not a real sha256": recipe(self.t / "b", binsum="SKIP"),
            "Maintainer": recipe(self.t / "c", maint="Someone <placeholder: set before publishing>"),
        }
        for want, d in cases.items():
            self.assertTrue(any(want in e for e in tr.check("2.0.0", d)), (want, tr.check("2.0.0", d)))
        d = recipe(self.t / "d")
        (d / ".SRCINFO").write_text((d / ".SRCINFO").read_text().replace("\tdepends = alsa-lib\n", ""))
        self.assertTrue(any(".SRCINFO doesn't match" in e for e in tr.check("2.0.0", d)))


class Pack(unittest.TestCase):
    def test_one_folder_two_files_reproducible(self):
        t = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, t)
        d = recipe(t / "r")
        (t / "o1").mkdir(), (t / "o2").mkdir()
        a, b = tr.pack(d, t / "o1", "2.0.0"), tr.pack(d, t / "o2", "2.0.0")
        self.assertEqual(a.name, "mouser-bin-2.0.0-taur.tar.gz")
        self.assertEqual(a.read_bytes(), b.read_bytes(), "reproducible")
        with tarfile.open(a) as tf:
            self.assertEqual(tf.getnames(), ["mouser-bin", "mouser-bin/PKGBUILD", "mouser-bin/.SRCINFO"])
            self.assertEqual(tf.extractfile("mouser-bin/.SRCINFO").read(), (d / ".SRCINFO").read_bytes())


class VerifyRelease(unittest.TestCase):
    def test_checksums_compared_with_the_downloads(self):
        t = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, t)
        self.assertEqual(tr.verify_release(recipe(t / "ok"), fake_fetch), [])
        bad = recipe(t / "bad", binsum="0" * 64)
        errs = tr.verify_release(bad, fake_fetch)
        self.assertEqual(len(errs), 1)
        self.assertIn("releases/download/v2.0.0", errs[0])
        self.assertTrue(tr.verify_release(recipe(t / "x"), lambda u: (_ for _ in ()).throw(OSError("offline")))[0].startswith("can't download"))


class Publish(unittest.TestCase):
    def setUp(self):
        self.t = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.t)
        self.ctl = self.t / "taurctl"
        self.ctl.write_text(FAKE)
        self.ctl.chmod(self.ctl.stat().st_mode | stat.S_IEXEC)
        self.log = self.t / "log"
        self.env = {k: os.environ.pop(k) for k in list(os.environ) if k.startswith("TAUR_FAKE")}
        os.environ["TAUR_FAKE_LOG"] = str(self.log)
        self.addCleanup(self.restore)
        self.d = recipe(self.t / "r")

    def restore(self):
        for k in [k for k in os.environ if k.startswith("TAUR_FAKE")]:
            del os.environ[k]
        os.environ.update(self.env)

    def publish(self, dry=False):
        out = io.StringIO()
        code = tr.publish("2.0.0", self.d, "WeirdBrains", dry, True, str(self.ctl), out=out, fetch=fake_fetch)
        return code, out.getvalue()

    def calls(self):
        return self.log.read_text().splitlines() if self.log.exists() else []

    def test_uploads_to_the_org_and_confirms_the_version(self):
        code, out = self.publish()
        self.assertEqual(code, 0)
        calls = self.calls()
        self.assertEqual(calls[0], "whoami")
        self.assertRegex(calls[1], r"^pkg upload \S+/mouser-bin-2\.0\.0-taur\.tar\.gz --org WeirdBrains --yes$")
        self.assertEqual(calls[2], "info mouser-bin --json")
        self.assertIn("TAUR has mouser-bin 2.0.0-1", out)
        self.assertEqual(Path(str(self.log) + ".tar").read_text().split(), ["mouser-bin/", "mouser-bin/PKGBUILD", "mouser-bin/.SRCINFO"])

    def test_dry_run_never_uploads(self):
        code, out = self.publish(dry=True)
        self.assertEqual(code, 0)
        self.assertEqual(self.calls(), ["whoami"])
        self.assertIn("[dry-run] ", out)
        self.assertIn("pkg upload mouser-bin-2.0.0-taur.tar.gz --org WeirdBrains --yes", out)

    def test_not_signed_in_stops_before_upload(self):
        os.environ["TAUR_FAKE_OUT"] = "1"
        with self.assertRaisesRegex(tr.TaurError, "not signed in"):
            self.publish()
        self.assertEqual(self.calls(), ["whoami"])

    def test_upload_refusal_and_wrong_version_fail(self):
        os.environ["TAUR_FAKE_UPLOAD_FAIL"] = "1"
        with self.assertRaisesRegex(tr.TaurError, "upload failed: rejected"):
            self.publish()
        del os.environ["TAUR_FAKE_UPLOAD_FAIL"]
        os.environ["TAUR_FAKE_VERSION"] = "1.6.0-1"
        with self.assertRaisesRegex(tr.TaurError, "TAUR reports mouser-bin '1.6.0-1'"):
            self.publish()

    def test_a_bad_recipe_or_release_never_reaches_taurctl(self):
        (self.d / "PKGBUILD").write_text((self.d / "PKGBUILD").read_text().replace("pkgver=2.0.0", "pkgver=1.0.0"))
        with self.assertRaisesRegex(tr.TaurError, "recipe not ready"):
            self.publish()
        self.d = recipe(self.t / "r2", binsum="1" * 64)
        with self.assertRaisesRegex(tr.TaurError, "release doesn't match"):
            self.publish()
        self.assertEqual(self.calls(), [])

    def test_cli_exit_codes(self):
        with contextlib.redirect_stderr(io.StringIO()), contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(tr.main(["check", "2.0.0", str(self.d)]), 0)
            self.assertEqual(tr.main(["check", "9.9.9", str(self.d)]), 1)


if __name__ == "__main__":
    unittest.main()
