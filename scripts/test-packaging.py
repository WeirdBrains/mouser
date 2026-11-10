#!/usr/bin/env python3
"""Tests for scripts/packaging.py: the Homebrew formula and cask get a real
sha256 for every architecture or nothing is written (no network).

    python3 scripts/test-packaging.py [-v]
"""
import contextlib, hashlib, importlib.util, io, shutil, subprocess, sys, tempfile, unittest
from pathlib import Path

sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location("packaging", Path(__file__).resolve().parent / "packaging.py")
pk = importlib.util.module_from_spec(spec)
spec.loader.exec_module(pk)

NAMES = [
    "mouser-aarch64-apple-darwin.tar.gz",
    "mouser-x86_64-apple-darwin.tar.gz",
    "Mouser-aarch64-apple-darwin.dmg",
    "Mouser-x86_64-apple-darwin.dmg",
]


class Brew(unittest.TestCase):
    def setUp(self):
        self.t = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.t)
        self.root, self.dist = self.t / "repo", self.t / "dist"
        self.root.mkdir()
        # As actions/download-artifact lays them out: one folder per artifact.
        for n in NAMES:
            d = self.dist / n.split(".")[0]
            d.mkdir(parents=True, exist_ok=True)
            (d / n).write_bytes(n.encode())
            (d / (n + ".sha256")).write_text(f"{hashlib.sha256(n.encode()).hexdigest()}  {n}\n")

    def run_main(self, *args):
        err = io.StringIO()
        with contextlib.redirect_stderr(err), contextlib.redirect_stdout(io.StringIO()):
            code = pk.main(list(args))
        return code, err.getvalue()

    def test_every_architecture_gets_its_hash(self):
        self.assertEqual(self.run_main("brew", "v1.5.0", "--assets", str(self.dist), "--root", str(self.root))[0], 0)
        f = (self.root / "Formula/mouser.rb").read_text()
        c = (self.root / "Casks/mouser.rb").read_text()
        for n in NAMES:
            text = f if n.endswith(".tar.gz") else c
            self.assertIn(f'sha256 "{hashlib.sha256(n.encode()).hexdigest()}"', text)
            self.assertIn(f"/releases/download/v1.5.0/{n}", text)
        self.assertNotIn('sha256 ""', f + c)
        self.assertEqual(self.run_main("check", str(self.root)), (0, ""))
        if shutil.which("ruby"):
            for p in ("Formula/mouser.rb", "Casks/mouser.rb"):
                subprocess.run(["ruby", "-c", str(self.root / p)], check=True, capture_output=True)

    def test_a_missing_asset_writes_nothing(self):
        next(self.dist.rglob("Mouser-x86_64-apple-darwin.dmg")).unlink()
        code, err = self.run_main("brew", "1.5.0", "--assets", str(self.dist), "--root", str(self.root))
        self.assertEqual(code, 1)
        self.assertIn("Mouser-x86_64-apple-darwin.dmg is not in", err)
        self.assertFalse((self.root / "Formula").exists() or (self.root / "Casks").exists())

    def test_a_file_that_does_not_match_its_sha256_is_refused(self):
        next(self.dist.rglob("mouser-aarch64-apple-darwin.tar.gz")).write_bytes(b"changed")
        code, err = self.run_main("brew", "1.5.0", "--assets", str(self.dist), "--root", str(self.root))
        self.assertEqual(code, 1)
        self.assertIn("does not match its .sha256", err)

    def test_two_different_copies_are_refused(self):
        (self.dist / "again").mkdir()
        (self.dist / "again/Mouser-aarch64-apple-darwin.dmg").write_bytes(b"other")
        code, err = self.run_main("brew", "1.5.0", "--assets", str(self.dist), "--root", str(self.root))
        self.assertEqual(code, 1)
        self.assertIn("different contents", err)

    def test_check_catches_empty_hashes_and_wrong_urls(self):
        (self.root / "Formula").mkdir()
        (self.root / "Casks").mkdir()
        good = pk.render("1.5.0", {k: "a" * 64 for k, _ in pk.assets()})
        (self.root / "Formula/mouser.rb").write_text(good[0].replace("a" * 64, "", 1))
        (self.root / "Casks/mouser.rb").write_text(good[1].replace("v1.5.0/Mouser-x86_64", "v1.4.0/Mouser-x86_64"))
        problems = pk.check(self.root)
        self.assertEqual(len(problems), 2, problems)
        self.assertIn("formula arm: sha256 is ''", problems[0])
        self.assertIn("cask intel: url", problems[1])
        with self.assertRaises(pk.PackagingError):
            pk.render("1.5.0", {k: "" for k, _ in pk.assets()})

    def test_the_committed_files_are_sound(self):
        self.assertEqual(pk.check(Path(__file__).resolve().parent.parent), [])


if __name__ == "__main__":
    unittest.main()
