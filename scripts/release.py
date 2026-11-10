#!/usr/bin/env python3
"""Cut a Mouser release: bump the version, add the CHANGELOG entry, commit
`release: vX.Y.Z`, create the annotated tag `vX.Y.Z` and push main with it.

    release.py [--major | --patch] [--section NAME] [--dry-run] [--no-push]
               [--no-fetch] [--expect-head SHA] [--date YYYY-MM-DD] MESSAGE

Bumps MINOR by default (1.4.5 -> 1.5.0); --patch gives 1.4.6, --major 2.0.0.
MESSAGE goes into CHANGELOG.md (one sentence becomes one bullet under
--section, default Added, or Fixed with --patch; markdown `- ` bullets and
`### Heading` blocks are used as written) and into the commit body. Existing
`## [Unreleased]` entries are folded into the new section, dated in
America/New_York, and an empty `## [Unreleased]` is left on top.

Tag-time bumps (what earlier `release:` commits changed): the workspace
version in Cargo.toml, crates/mouser-llm (and any member pinned to the
workspace version), Cargo.lock when present (it is gitignored), the
version lines of man/*.1, "Current release" in docs/HANDOVER.md, and the
homepage version and install URLs in site_jaspr/lib/pages/home.dart. The
Homebrew formula/cask and the mouser-bin PKGBUILD need the release's
checksums and are updated after the Release workflow, not here.

Pushing the `v*` tag is what starts the Release and CI workflows. A push
to main deploys the website when the commit touches the site or docs.
Refuses (exit 1, nothing written) off main, with a dirty
tree, when main is behind origin/main, when the tag exists, or when HEAD is
not --expect-head. --dry-run prints every step and changes nothing;
--no-push commits and tags without pushing. Python 3.8+. docs/release.md.
"""
import argparse, datetime, difflib, os, re, subprocess, sys
from pathlib import Path

ORDER = ["Added", "Changed", "Deprecated", "Removed", "Fixed", "Security"]
# Members with their own `version = "..."` that every release sets to the app
# version even if it drifted (members pinned to the workspace version follow too).
FOLLOW = ("crates/mouser-llm",)
SEMVER = re.compile(r"^(\d+)\.(\d+)\.(\d+)$")


class ReleaseError(Exception):
    pass


def bump(version, part="minor"):
    m = SEMVER.match(version)
    if not m:
        raise ReleaseError(f"version {version!r} is not MAJOR.MINOR.PATCH")
    major, minor, patch = map(int, m.groups())
    if part == "major":
        return f"{major + 1}.0.0"
    if part == "patch":
        return f"{major}.{minor}.{patch + 1}"
    return f"{major}.{minor + 1}.0"


def today_new_york():
    try:
        from zoneinfo import ZoneInfo  # 3.9+
        return datetime.datetime.now(ZoneInfo("America/New_York")).date().isoformat()
    except Exception:
        out = subprocess.run(["date", "+%Y-%m-%d"], env=dict(os.environ, TZ="America/New_York"),
                             capture_output=True, text=True)
        if out.returncode != 0 or not re.match(r"^\d{4}-\d{2}-\d{2}$", out.stdout.strip()):
            raise ReleaseError("cannot get today's date in America/New_York; pass --date")
        return out.stdout.strip()


# ---- CHANGELOG ------------------------------------------------------------

def _blocks(text):
    """Split changelog body text into (preamble items, {heading: items}) keeping
    heading order. An item is one bullet with its continuation lines."""
    pre, heads, cur = [], {}, None
    item = None

    def close():
        nonlocal item
        if item is not None:
            body = "\n".join(item).rstrip()
            if body:
                (pre if cur is None else heads[cur]).append(body)
        item = None

    for line in text.splitlines():
        h = re.match(r"^###\s+(.+?)\s*$", line)
        if h:
            close()
            cur = h.group(1)
            heads.setdefault(cur, [])
        elif re.match(r"^[-*]\s+", line):
            close()
            item = ["- " + re.sub(r"^[-*]\s+", "", line)]
        elif not line.strip():
            if item is not None:
                item.append("")
        else:
            if item is None:
                item = [line]
            else:
                item.append(line)
    close()
    return pre, heads


def message_blocks(message, section):
    """The release message as changelog items: `### Heading` blocks and `- `
    bullets are used as written; plain text becomes one bullet."""
    text = message.strip()
    if not text:
        raise ReleaseError("the release message is empty")
    if re.search(r"^###\s", text, re.M) or re.match(r"^[-*]\s", text):
        pre, heads = _blocks(text)
        if pre:
            heads = dict([(section, pre + heads.pop(section, []))] + list(heads.items()))
        return heads
    return {section: ["- " + " ".join(text.split())]}


def merge(old, new):
    out = {h: list(v) for h, v in old.items()}
    for h, items in new.items():
        dst = out.setdefault(h, [])
        for it in items:
            if it not in dst:
                dst.append(it)
    known = [h for h in old]  # keep the Unreleased order, then Keep a Changelog order
    rest = sorted((h for h in out if h not in old), key=lambda h: ORDER.index(h) if h in ORDER else len(ORDER))
    return {h: out[h] for h in known + rest if out[h]}


def rewrite_changelog(text, new_version, date, message, section):
    m = re.search(r"^## \[Unreleased\][^\n]*\n", text, re.M)
    if not m:
        raise ReleaseError("CHANGELOG.md has no `## [Unreleased]` heading")
    if re.search(r"^## \[" + re.escape(new_version) + r"\]", text, re.M):
        raise ReleaseError(f"CHANGELOG.md already has a [{new_version}] section")
    nxt = re.search(r"^## \[", text[m.end():], re.M)
    end = m.end() + nxt.start() if nxt else len(text)
    body = text[m.end():end]
    pre, heads = _blocks(body)
    heads = merge(heads, message_blocks(message, section))
    parts = [f"## [Unreleased]\n\n## [{new_version}] - {date}\n"]
    if pre:
        parts.append("\n" + "\n\n".join(pre) + "\n")
    for h, items in heads.items():
        parts.append(f"\n### {h}\n\n" + "\n".join(items) + "\n")
    section_text = "".join(parts) + ("\n" if nxt else "")
    return text[:m.start()] + section_text + text[end:]


def changelog_section(text, version):
    m = re.search(r"^## \[" + re.escape(version) + r"\][^\n]*\n", text, re.M)
    nxt = re.search(r"^## \[", text[m.end():], re.M)
    return text[m.start():m.end() + nxt.start() if nxt else len(text)].rstrip() + "\n"


# ---- version files --------------------------------------------------------

def workspace_version(cargo_toml):
    m = re.search(r"^\[workspace\.package\][^\[]*?^version\s*=\s*\"([^\"]+)\"", cargo_toml, re.M | re.S)
    if not m:
        raise ReleaseError("Cargo.toml has no [workspace.package] version")
    return m.group(1)


def bump_workspace_toml(text, old, new):
    pat = re.compile(r"(^\[workspace\.package\][^\[]*?^version\s*=\s*\")" + re.escape(old) + r"\"", re.M | re.S)
    out, n = pat.subn(lambda m: m.group(1) + new + '"', text, count=1)
    if n != 1:
        raise ReleaseError("could not bump the [workspace.package] version in Cargo.toml")
    return out


def members(cargo_toml):
    m = re.search(r"^members\s*=\s*\[(.*?)\]", cargo_toml, re.M | re.S)
    if not m:
        return []
    body = re.sub(r"#[^\n]*", "", m.group(1))
    return re.findall(r"\"([^\"]+)\"", body)


def package_version_line(text):
    """('workspace', None) for version.workspace = true, ('pinned', v) for a
    literal version under [package], else (None, None)."""
    m = re.search(r"^\[package\](.*?)(?=^\[|\Z)", text, re.M | re.S)
    if not m:
        return None, None, None
    sec = m.group(1)
    name = re.search(r"^name\s*=\s*\"([^\"]+)\"", sec, re.M)
    name = name.group(1) if name else None
    if re.search(r"^version(\.workspace\s*=\s*true|\s*=\s*\{\s*workspace\s*=\s*true\s*\})", sec, re.M):
        return name, "workspace", None
    v = re.search(r"^version\s*=\s*\"([^\"]+)\"", sec, re.M)
    return (name, "pinned", v.group(1)) if v else (name, None, None)


def bump_pinned(text, old, new):
    pat = re.compile(r"(^\[package\][^\[]*?^version\s*=\s*\")" + re.escape(old) + r"\"", re.M | re.S)
    out, n = pat.subn(lambda m: m.group(1) + new + '"', text, count=1)
    if n != 1:
        raise ReleaseError("could not bump a pinned crate version")
    return out


def bump_lock(text, names, new):
    def one(m):
        if m.group(2) in names:
            return m.group(1) + new + '"'
        return m.group(0)
    return re.sub(r"(\[\[package\]\]\nname = \"([^\"]+)\"\nversion = \")[^\"]+\"", one, text)


def bump_man(text, new):
    """The `.TH name 1 "name X.Y.Z"` title and the `vX.Y.Z` line under
    `.SH VERSION`, whatever version they carry now."""
    out = re.sub(r'^(\.TH .*"\S+ )\d+\.\d+\.\d+"', lambda m: m.group(1) + new + '"', text, flags=re.M)
    return re.sub(r"(^\.SH VERSION\n(?:\..*\n)*)v\d+\.\d+\.\d+$", lambda m: m.group(1) + "v" + new, out, flags=re.M)


def bump_handover(text, new, date):
    out, n = re.subn(r"Current release: v[0-9][0-9A-Za-z.\-]* \(\d{4}-\d{2}-\d{2}\)",
                     f"Current release: v{new} ({date})", text, count=1)
    if n != 1:
        raise ReleaseError("docs/HANDOVER.md has no `Current release: vX.Y.Z (YYYY-MM-DD)`")
    return out


def bump_home_dart(text, old, new):
    """Replace the homepage's current version (hero link and install URLs)
    with `new`. Uses the `releases/tag/vX.Y.Z` href, so a drifted
    homepage still follows the release; `old` is kept for the same
    signature as the other bump helpers."""
    m = re.search(r"releases/tag/v(\d+\.\d+\.\d+)", text)
    if not m:
        raise ReleaseError("site_jaspr/lib/pages/home.dart has no releases/tag/vX.Y.Z")
    current = m.group(1)
    if current == new:
        return text
    return text.replace(current, new)


def plan_edits(root, part, message, section, date):
    """Every file the release rewrites, as {relative path: (old, new)}, plus the
    old and new versions. Reads only; raises before anything is written."""
    root = Path(root)
    edits = {}

    def read(rel):
        return (root / rel).read_text(encoding="utf-8")

    def put(rel, old_text, new_text):
        if old_text != new_text:
            edits[rel] = (old_text, new_text)

    cargo = read("Cargo.toml")
    old = workspace_version(cargo)
    new = bump(old, part)
    put("Cargo.toml", cargo, bump_workspace_toml(cargo, old, new))

    lock_names = set()
    for mem in members(cargo):
        rel = f"{mem}/Cargo.toml"
        if not (root / rel).is_file():
            continue
        text = read(rel)
        name, kind, v = package_version_line(text)
        if kind == "workspace":
            lock_names.add(name)
        elif kind == "pinned" and (v == old or mem in FOLLOW):
            put(rel, text, bump_pinned(text, v, new))
            lock_names.add(name)

    if (root / "Cargo.lock").is_file():
        text = read("Cargo.lock")
        put("Cargo.lock", text, bump_lock(text, lock_names, new))

    man = root / "man"
    if man.is_dir():
        for p in sorted(man.glob("*.1")):
            rel = p.relative_to(root).as_posix()
            text = read(rel)
            put(rel, text, bump_man(text, new))

    if (root / "docs/HANDOVER.md").is_file():
        text = read("docs/HANDOVER.md")
        put("docs/HANDOVER.md", text, bump_handover(text, new, date))

    rel = "site_jaspr/lib/pages/home.dart"
    if not (root / rel).is_file():
        raise ReleaseError(f"{rel} is missing (the homepage version lives on main)")
    text = read(rel)
    put(rel, text, bump_home_dart(text, old, new))

    text = read("CHANGELOG.md")
    put("CHANGELOG.md", text, rewrite_changelog(text, new, date, message, section))
    return old, new, edits


# ---- git ------------------------------------------------------------------

def git(root, *args, check=True):
    r = subprocess.run(["git", "-C", str(root)] + list(args), capture_output=True, text=True)
    if check and r.returncode != 0:
        raise ReleaseError(f"git {' '.join(args)} failed: {(r.stderr or r.stdout).strip()}")
    return r


def preflight(root, tag, expect_head, fetch):
    branch = git(root, "symbolic-ref", "--quiet", "--short", "HEAD", check=False).stdout.strip()
    if branch != "main":
        raise ReleaseError(f"not on main (on {branch or 'a detached HEAD'}); releases are cut from main")
    dirty = git(root, "status", "--porcelain", "--untracked-files=no").stdout.strip()
    if dirty:
        raise ReleaseError("the working tree has uncommitted changes:\n" + dirty)
    head = git(root, "rev-parse", "HEAD").stdout.strip()
    if expect_head and not head.startswith(expect_head.lower()):
        raise ReleaseError(f"HEAD is {head[:12]}, not --expect-head {expect_head}")
    if fetch:
        git(root, "fetch", "--quiet", "origin", "main")
        if git(root, "ls-remote", "--tags", "origin", f"refs/tags/{tag}").stdout.strip():
            raise ReleaseError(f"tag {tag} already exists on origin")
    remote = git(root, "rev-parse", "--verify", "--quiet", "refs/remotes/origin/main", check=False).stdout.strip()
    if remote and git(root, "merge-base", "--is-ancestor", remote, head, check=False).returncode != 0:
        raise ReleaseError("main is behind or has diverged from origin/main; pull (rebase) first")
    if git(root, "rev-parse", "--verify", "--quiet", f"refs/tags/{tag}", check=False).returncode == 0:
        raise ReleaseError(f"tag {tag} already exists")
    return head


def tracked(root, rel):
    return git(root, "ls-files", "--error-unmatch", rel, check=False).returncode == 0


def main(argv=None):
    ap = argparse.ArgumentParser(prog="release.py", description=__doc__.split("\n\n")[0],
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("message", help="release summary for CHANGELOG.md and the commit body")
    g = ap.add_mutually_exclusive_group()
    g.add_argument("--major", action="store_true", help="bump MAJOR (X.0.0)")
    g.add_argument("--patch", action="store_true", help="bump PATCH (x.y.Z)")
    ap.add_argument("--section", choices=ORDER, help="CHANGELOG heading for a plain message (default Added; Fixed with --patch)")
    ap.add_argument("--dry-run", action="store_true", help="print every step and change nothing")
    ap.add_argument("--no-push", action="store_true", help="commit and tag but do not push")
    ap.add_argument("--no-fetch", action="store_true", help="skip fetching origin (offline checks only)")
    ap.add_argument("--expect-head", metavar="SHA", help="refuse unless HEAD is this commit")
    ap.add_argument("--date", help="release date (default: today in America/New_York)")
    ap.add_argument("--repo", default=None, help=argparse.SUPPRESS)
    a = ap.parse_args(argv)

    part = "major" if a.major else "patch" if a.patch else "minor"
    section = a.section or ("Fixed" if a.patch else "Added")
    root = Path(a.repo) if a.repo else Path(__file__).resolve().parent.parent
    dry = "[dry-run] " if a.dry_run else ""
    try:
        if not a.message.strip():
            raise ReleaseError("the release message is empty")
        date = a.date or today_new_york()
        if not re.match(r"^\d{4}-\d{2}-\d{2}$", date):
            raise ReleaseError(f"--date {date!r} is not YYYY-MM-DD")
        root = Path(git(root, "rev-parse", "--show-toplevel").stdout.strip())
        old, new, edits = plan_edits(root, part, a.message, section, date)
        tag = f"v{new}"
        head = preflight(root, tag, a.expect_head, fetch=not a.no_fetch)
        commit_msg = f"release: {tag}\n\n{a.message.strip()}\n"

        print(f"{dry}{old} -> {new} ({part}) on main at {head[:12]}")
        man = [p for p in edits if p.startswith("man/")]
        for rel in edits:
            if not rel.startswith("man/"):
                note = "" if rel != "Cargo.lock" or tracked(root, rel) else " (not tracked; updated, not committed)"
                print(f"{dry}write {rel}{note}")
        if man:
            print(f"{dry}write {len(man)} man pages (man/*.1 version lines)")
        if a.dry_run:
            for rel, (o, n) in edits.items():
                if rel.startswith("man/") and rel != man[0]:
                    continue
                sys.stdout.writelines(difflib.unified_diff(o.splitlines(True), n.splitlines(True), f"a/{rel}", f"b/{rel}", n=1))
            print("\n[dry-run] CHANGELOG section:\n" + changelog_section(edits["CHANGELOG.md"][1], new))
            print("[dry-run] commit message:\n" + commit_msg)
            print(f"[dry-run] git tag -a {tag} -m {tag}")
            print("[dry-run] " + ("(no push)" if a.no_push else "git push origin main --follow-tags  # the v* tag starts the Release workflow"))
            return 0

        # Write everything, commit; on any failure put the files back.
        stage = [rel for rel in edits if tracked(root, rel)]
        try:
            for rel, (_, n) in edits.items():
                with open(root / rel, "w", encoding="utf-8", newline="") as f:
                    f.write(n)
            if git(root, "rev-parse", "HEAD").stdout.strip() != head:
                raise ReleaseError("HEAD moved while preparing the release")
            git(root, "add", "--", *stage)
            r = subprocess.run(["git", "-C", str(root), "commit", "--quiet", "-F", "-"], input=commit_msg,
                               capture_output=True, text=True)
            if r.returncode != 0:
                raise ReleaseError(f"git commit failed: {(r.stderr or r.stdout).strip()}")
        except BaseException:
            git(root, "reset", "--quiet", "HEAD", "--", *stage, check=False)
            for rel, (o, _) in edits.items():
                with open(root / rel, "w", encoding="utf-8", newline="") as f:
                    f.write(o)
            raise
        commit = git(root, "rev-parse", "HEAD").stdout.strip()
        if git(root, "rev-parse", "HEAD~1").stdout.strip() != head:
            raise ReleaseError(f"the release commit {commit[:12]} is not on top of {head[:12]}; not tagging")
        print(f"committed {commit[:12]} release: {tag}")
        git(root, "tag", "-a", tag, "-m", tag)
        print(f"tagged {tag}")
        if a.no_push:
            print("not pushed; push with: git push origin main --follow-tags")
            return 0
        r = subprocess.run(["git", "-C", str(root), "push", "origin", "main", "--follow-tags"], text=True)
        if r.returncode != 0:
            raise ReleaseError(f"push failed; {tag} and the commit are local. Retry: git push origin main --follow-tags")
        print(f"pushed main and {tag}; the tag push starts the Release workflow")
        return 0
    except ReleaseError as e:
        print(f"release: {e}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
