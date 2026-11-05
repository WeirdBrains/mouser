#!/usr/bin/env python3
"""Fail if a docs/*.md file has no website section in docs/sections.toml."""
import pathlib, sys, tomllib
root = pathlib.Path(__file__).resolve().parent.parent / "docs"
m = tomllib.loads((root / "sections.toml").read_text())["docs"]
ok = {"index", "user", "developer", "mcp", "api"}
docs = {p.relative_to(root).as_posix() for p in root.rglob("*.md") if "historical" not in p.parts}
errs = [f"docs/{d}: no section in docs/sections.toml" for d in sorted(docs - m.keys())]
errs += [f"docs/sections.toml: {d} does not exist" for d in sorted(m.keys() - docs)]
errs += [f"docs/sections.toml: {d} has unknown section {s!r}" for d, s in m.items() if s not in ok]
print("\n".join(errs) or f"{len(docs)} docs mapped to site sections")
sys.exit(1 if errs else 0)
