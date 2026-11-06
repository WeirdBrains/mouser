#!/usr/bin/env python3
"""Fail if a docs/*.md file has no website section in docs/sections.toml.

Public sections (user, developer, mcp, api) each get a page on the site.
"internal" is for maintainer/agent docs that stay in the repo only.
"index" is docs/README.md (not a page). docs/historical/ is the archive.
"""
import pathlib, sys, tomllib
root = pathlib.Path(__file__).resolve().parent.parent / "docs"
m = tomllib.loads((root / "sections.toml").read_text())["docs"]
ok = {"index", "user", "developer", "mcp", "api", "internal"}
docs = {p.relative_to(root).as_posix() for p in root.rglob("*.md") if "historical" not in p.parts}
errs = [f"docs/{d}: no section in docs/sections.toml" for d in sorted(docs - m.keys())]
errs += [f"docs/sections.toml: {d} does not exist" for d in sorted(m.keys() - docs)]
errs += [f"docs/sections.toml: {d} has unknown section {s!r}" for d, s in m.items() if s not in ok]
pub = sum(1 for s in m.values() if s in {"user", "developer", "mcp", "api"})
internal = sum(1 for s in m.values() if s == "internal")
print("\n".join(errs) or f"{len(docs)} docs mapped ({pub} public, {internal} internal, rest index)")
sys.exit(1 if errs else 0)
