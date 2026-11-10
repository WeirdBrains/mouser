#!/usr/bin/env python3
"""Tests for scripts/release.py: version bumps, CHANGELOG rewriting and the
git flow, in temporary repos with a local bare `origin` (no network).

    python3 scripts/test-release.py [-v]

Python 3.8+, needs git.
"""
import contextlib, importlib.util, io, os, subprocess, sys, tempfile, unittest
from pathlib import Path

sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location("release", Path(__file__).resolve().parent / "release.py")
release = importlib.util.module_from_spec(spec)
spec.loader.exec_module(release)

CHANGELOG = """## [Unreleased]

### Fixed

- ssh: hosts without the terminfo get xterm-256color.

## [1.4.5] - 2026-11-06

### Added

- Tab completion.
"""
CARGO = """[workspace]
members = [
  "crates/app",
  # pinned to the workspace version
  "crates/mouser-llm",
  "crates/core",
]

[workspace.package]
edition = "2024"
version = "1.4.5"

[workspace.dependencies]
log = "0.4.22"
"""
FILES = {
    "Cargo.toml": CARGO,
    "crates/app/Cargo.toml": '[package]\nname = "app"\nversion.workspace = true\n\n[dependencies]\nlog = { version = "0.4.22" }\n',
    "crates/mouser-llm/Cargo.toml": '[package]\nname = "mouser-llm"\nversion = "1.4.5"\n',
    "crates/core/Cargo.toml": '[package]\nname = "core"\nversion = "0.4.0"\n',
    "man/mouser.1": '.TH mouser 1  "mouser 1.4.5" \n.SH NAME\nmouser\n.SH VERSION\nv1.4.5\n',
    "docs/HANDOVER.md": "# Notes\n\nRepo. Current release: v1.4.5 (2026-11-06). CI.\n",
    "CHANGELOG.md": CHANGELOG,
    ".gitignore": "Cargo.lock\n",
}
LOCK = ('[[package]]\nname = "app"\nversion = "1.4.4"\n\n[[package]]\nname = "core"\nversion = "0.4.0"\n\n'
        '[[package]]\nname = "mouser-llm"\nversion = "1.4.5"\n\n[[package]]\nname = "log"\nversion = "1.4.5"\n')
ENV = dict(os.environ, GIT_CONFIG_GLOBAL=os.devnull, GIT_CONFIG_NOSYSTEM="1",
           GIT_AUTHOR_NAME="T", GIT_AUTHOR_EMAIL="t@example.com",
           GIT_COMMITTER_NAME="T", GIT_COMMITTER_EMAIL="t@example.com")


def sh(cwd, *args):
    return subprocess.run(["git", "-C", str(cwd)] + list(args), env=ENV, check=True,
                          capture_output=True, text=True).stdout.strip()


class Bump(unittest.TestCase):
    def test_parts(self):
        self.assertEqual(release.bump("1.4.5"), "1.5.0")
        self.assertEqual(release.bump("1.4.5", "patch"), "1.4.6")
        self.assertEqual(release.bump("1.4.5", "major"), "2.0.0")
        self.assertEqual(release.bump("0.9.12", "minor"), "0.10.0")

    def test_rejects_non_semver(self):
        for v in ("1.4", "1.4.5-rc1", "v1.4.5"):
            with self.assertRaises(release.ReleaseError):
                release.bump(v)

    def test_cargo_and_lock(self):
        self.assertEqual(release.workspace_version(CARGO), "1.4.5")
        out = release.bump_workspace_toml(CARGO, "1.4.5", "1.5.0")
        self.assertIn('version = "1.5.0"', out)
        self.assertIn('log = "0.4.22"', out)
        self.assertEqual(release.members(CARGO), ["crates/app", "crates/mouser-llm", "crates/core"])
        lock = release.bump_lock(LOCK, {"app", "mouser-llm"}, "1.5.0")
        self.assertIn('name = "app"\nversion = "1.5.0"', lock)
        self.assertIn('name = "mouser-llm"\nversion = "1.5.0"', lock)
        self.assertIn('name = "core"\nversion = "0.4.0"', lock)
        self.assertIn('name = "log"\nversion = "1.4.5"', lock)

    def test_man_and_handover(self):
        man = release.bump_man(FILES["man/mouser.1"], "1.5.0")
        self.assertIn('"mouser 1.5.0"', man)
        self.assertIn("\nv1.5.0\n", man)
        self.assertNotIn("1.4.5", man)
        drift = '.TH mouser-x 1  "mouser-x 1.4.4" \n.SH VERSION\n.ie x\nv1.4.4\n.SH OTHER\nv1.0.0\n'
        self.assertEqual(release.bump_man(drift, "1.5.0"), '.TH mouser-x 1  "mouser-x 1.5.0" \n.SH VERSION\n.ie x\nv1.5.0\n.SH OTHER\nv1.0.0\n')
        h = release.bump_handover(FILES["docs/HANDOVER.md"], "1.5.0", "2026-11-09")
        self.assertIn("Current release: v1.5.0 (2026-11-09).", h)


class Changelog(unittest.TestCase):
    def rw(self, message, section="Added", text=CHANGELOG):
        return release.rewrite_changelog(text, "1.5.0", "2026-11-09", message, section)

    def test_sentence_folds_unreleased(self):
        out = self.rw("Release script that bumps and tags.")
        self.assertTrue(out.startswith("## [Unreleased]\n\n## [1.5.0] - 2026-11-09\n\n### Fixed\n\n- ssh:"))
        self.assertIn("### Added\n\n- Release script that bumps and tags.\n\n## [1.4.5] - 2026-11-06", out)
        self.assertEqual(out.count("## [Unreleased]"), 1)
        self.assertTrue(out.endswith(CHANGELOG[CHANGELOG.index("## [1.4.5]"):]))

    def test_multiline_sentence_is_one_bullet(self):
        out = self.rw("Fix the thing\nwhen it breaks.", "Fixed")
        self.assertIn("### Fixed\n\n- ssh: hosts without the terminfo get xterm-256color.\n- Fix the thing when it breaks.\n", out)

    def test_markdown_bullets_and_headings(self):
        out = self.rw("### Changed\n\n- Faster start.\n\n### Fixed\n\n- Crash on start.\n")
        sec = release.changelog_section(out, "1.5.0")
        self.assertEqual(sec, "## [1.5.0] - 2026-11-09\n\n### Fixed\n\n- ssh: hosts without the terminfo get xterm-256color.\n"
                              "- Crash on start.\n\n### Changed\n\n- Faster start.\n")
        out = self.rw("- One.\n- Two.", "Added")
        self.assertIn("### Added\n\n- One.\n- Two.\n", out)

    def test_duplicate_bullet_not_repeated(self):
        out = self.rw("- ssh: hosts without the terminfo get xterm-256color.", "Fixed")
        self.assertEqual(out.count("xterm-256color"), 1)

    def test_empty_unreleased(self):
        text = "## [Unreleased]\n\n## [1.4.5] - 2026-11-06\n\n- x\n"
        out = self.rw("New.", "Added", text)
        self.assertEqual(out, "## [Unreleased]\n\n## [1.5.0] - 2026-11-09\n\n### Added\n\n- New.\n\n## [1.4.5] - 2026-11-06\n\n- x\n")

    def test_refusals(self):
        with self.assertRaises(release.ReleaseError):
            self.rw("x", text="## [1.4.5]\n")
        with self.assertRaises(release.ReleaseError):
            release.rewrite_changelog(CHANGELOG, "1.4.5", "2026-11-09", "x", "Added")
        with self.assertRaises(release.ReleaseError):
            self.rw("   ")


class Flow(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        t = Path(self.tmp.name)
        self.origin, self.repo = t / "origin.git", t / "work"
        subprocess.run(["git", "init", "-q", "--bare", "-b", "main", str(self.origin)], env=ENV, check=True)
        subprocess.run(["git", "init", "-q", "-b", "main", str(self.repo)], env=ENV, check=True)
        for rel, text in FILES.items():
            p = self.repo / rel
            p.parent.mkdir(parents=True, exist_ok=True)
            p.write_text(text)
        (self.repo / "Cargo.lock").write_text(LOCK)
        sh(self.repo, "add", "-A")
        sh(self.repo, "commit", "-q", "-m", "init")
        sh(self.repo, "remote", "add", "origin", str(self.origin))
        sh(self.repo, "push", "-q", "-u", "origin", "main")
        self.head = sh(self.repo, "rev-parse", "HEAD")

    def tearDown(self):
        self.tmp.cleanup()

    def run_release(self, *args):
        out, err = io.StringIO(), io.StringIO()
        old = dict(os.environ)
        os.environ.update(ENV)
        try:
            with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
                try:
                    rc = release.main(["--repo", str(self.repo), "--date", "2026-11-09"] + list(args))
                except SystemExit as e:
                    rc = e.code
        finally:
            os.environ.clear()
            os.environ.update(old)
        return rc, out.getvalue(), err.getvalue()

    def snapshot(self):
        return {p.relative_to(self.repo).as_posix(): p.read_bytes()
                for p in self.repo.rglob("*") if p.is_file() and ".git" not in p.parts}

    def test_dry_run_changes_nothing(self):
        before = self.snapshot()
        rc, out, _ = self.run_release("--dry-run", "Release script.")
        self.assertEqual(rc, 0)
        self.assertIn("1.4.5 -> 1.5.0 (minor)", out)
        self.assertIn("release: v1.5.0\n\nRelease script.", out)
        self.assertIn("git tag -a v1.5.0", out)
        self.assertIn("git push origin main --follow-tags", out)
        self.assertEqual(self.snapshot(), before)
        self.assertEqual(sh(self.repo, "rev-parse", "HEAD"), self.head)
        self.assertEqual(sh(self.repo, "tag"), "")

    def test_release_commits_tags_and_pushes(self):
        rc, out, err = self.run_release("--patch", "Crash on start.")
        self.assertEqual(rc, 0, err)
        self.assertEqual(sh(self.repo, "log", "-1", "--format=%s%n%n%b"), "release: v1.4.6\n\nCrash on start.")
        self.assertEqual(sh(self.repo, "cat-file", "-t", "v1.4.6"), "tag")
        self.assertEqual(sh(self.origin, "rev-parse", "main"), sh(self.repo, "rev-parse", "HEAD"))
        self.assertEqual(sh(self.origin, "rev-parse", "v1.4.6^{commit}"), sh(self.repo, "rev-parse", "HEAD"))
        files = sh(self.repo, "show", "--name-only", "--format=", "HEAD").split()
        self.assertEqual(sorted(files), ["CHANGELOG.md", "Cargo.toml", "crates/mouser-llm/Cargo.toml", "docs/HANDOVER.md", "man/mouser.1"])
        self.assertIn('version = "0.4.0"', (self.repo / "crates/core/Cargo.toml").read_text())
        lock = (self.repo / "Cargo.lock").read_text()
        self.assertIn('name = "app"\nversion = "1.4.6"', lock)
        cl = (self.repo / "CHANGELOG.md").read_text()
        self.assertIn("## [1.4.6] - 2026-11-09\n\n### Fixed\n\n- ssh: hosts without the terminfo get xterm-256color.\n- Crash on start.\n", cl)
        self.assertEqual(sh(self.repo, "status", "--porcelain"), "")

    def test_followed_files_bumped_after_hand_edit(self):
        # Cargo.toml bumped by hand: man pages and the pinned crate still follow.
        (self.repo / "Cargo.toml").write_text(CARGO.replace('version = "1.4.5"', 'version = "1.4.7"'))
        sh(self.repo, "commit", "-q", "-am", "version bump")
        sh(self.repo, "push", "-q", "origin", "main")
        rc, _, err = self.run_release("--patch", "--no-push", "x")
        self.assertEqual(rc, 0, err)
        self.assertIn("\nv1.4.8\n", (self.repo / "man/mouser.1").read_text())
        self.assertEqual(release.package_version_line((self.repo / "crates/mouser-llm/Cargo.toml").read_text())[2], "1.4.8")
        self.assertIn('"0.4.0"', (self.repo / "crates/core/Cargo.toml").read_text())

    def test_no_push_and_major(self):
        rc, _, err = self.run_release("--major", "--no-push", "Big.")
        self.assertEqual(rc, 0, err)
        self.assertEqual(sh(self.repo, "cat-file", "-t", "v2.0.0"), "tag")
        self.assertEqual(sh(self.origin, "rev-parse", "main"), self.head)
        self.assertEqual(sh(self.origin, "tag"), "")

    def assertRefused(self, *args, expect=""):
        before = self.snapshot()
        head = sh(self.repo, "rev-parse", "HEAD")
        rc, _, err = self.run_release(*args)
        self.assertNotEqual(rc, 0)
        self.assertIn(expect, err)
        self.assertEqual(self.snapshot(), before)
        self.assertEqual(sh(self.repo, "rev-parse", "HEAD"), head)
        return err

    def test_refuses_major_and_patch(self):
        self.assertRefused("--major", "--patch", "x", expect="not allowed with")

    def test_refuses_missing_or_empty_message(self):
        self.assertRefused(expect="message")
        self.assertRefused("  ", expect="empty")

    def test_refuses_off_main(self):
        sh(self.repo, "checkout", "-q", "-b", "topic")
        self.assertRefused("x", expect="not on main")

    def test_refuses_dirty_tree(self):
        (self.repo / "Cargo.toml").write_text(CARGO + "\n# edit\n")
        self.assertRefused("x", expect="uncommitted")

    def test_refuses_existing_tag(self):
        sh(self.repo, "tag", "-a", "v1.5.0", "-m", "v1.5.0")
        self.assertRefused("x", "--no-fetch", expect="already exists")

    def test_refuses_tag_on_origin(self):
        sh(self.repo, "tag", "-a", "v1.5.0", "-m", "v1.5.0")
        sh(self.repo, "push", "-q", "origin", "v1.5.0")
        sh(self.repo, "tag", "-d", "v1.5.0")
        self.assertRefused("x", expect="already exists on origin")

    def test_refuses_wrong_expected_head(self):
        self.assertRefused("--expect-head", "0000000", "x", expect="not --expect-head")
        rc, _, err = self.run_release("--dry-run", "--expect-head", self.head[:10], "x")
        self.assertEqual(rc, 0, err)

    def test_refuses_behind_origin(self):
        other = Path(self.tmp.name) / "other"
        subprocess.run(["git", "clone", "-q", str(self.origin), str(other)], env=ENV, check=True)
        (other / "README").write_text("x\n")
        sh(other, "add", "README")
        sh(other, "commit", "-q", "-m", "other")
        sh(other, "push", "-q", "origin", "main")
        self.assertRefused("x", expect="behind")

    def test_failed_commit_restores_files(self):
        hook = self.repo / ".git/hooks/pre-commit"
        hook.write_text("#!/bin/sh\nexit 1\n")
        hook.chmod(0o755)
        self.assertRefused("x", expect="git commit failed")
        self.assertEqual(sh(self.repo, "status", "--porcelain"), "")
        self.assertEqual(sh(self.repo, "tag"), "")


if __name__ == "__main__":
    unittest.main()
