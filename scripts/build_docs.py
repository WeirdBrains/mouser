#!/usr/bin/env python3
"""Build site/docs/ from main's docs/*.md and docs/sections.toml, plus site-src/ extras.

Usage: python3 scripts/build_docs.py --docs ../main/docs --out site/docs
Everything under --out is generated; edit docs/ on main (or site-src/ here) instead.
"""
import argparse, html, posixpath, pathlib, re, shutil, tomllib
import markdown

SECTIONS = {
    "user": ("User Guides", "Guides to using Mouser: its commands, modes, chat, themes and preferences."),
    "developer": ("Developer Guides", "Building plugins (such as a mode), rendering engines (such as a custom GPU engine) and Fuzzball integrations."),
    "mcp": ("MCP Guides", "Mouser's MCP server and client, and how to integrate with the server."),
    "api": ("API Guides", "The published mouser crates, for developers who want to use them."),
}
BADGE = {"draft": "Draft", "placeholder": "Placeholder", "coming-soon": "Coming soon"}

def front(text):
    meta = {}
    if text.startswith("---\n"):
        head, _, text = text[4:].partition("\n---\n")
        for line in head.splitlines():
            k, _, v = line.partition(":")
            meta[k.strip()] = v.strip()
    return meta, text

def title_of(text, fallback):
    m = re.search(r"^# (.+)$", text, re.M)
    return m.group(1).strip() if m else fallback

# Every page() body sits in <section class="light doc">; style.css alternates its h1/h2
# headers through the front page's dark/light section bands, so new docs get it automatically.
def page(title, body, depth, active=None, note=""):
    up = "../" * depth
    nav = "".join(
        f'<a href="{up}docs/{k}/"{" aria-current=page" if k == active else ""}>{v[0]}</a>'
        for k, v in SECTIONS.items())
    return f"""<!doctype html>
<html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{html.escape(title)} — Mouser docs</title>
<link rel="icon" href="{up}assets/icon.png"><link rel="stylesheet" href="{up}style.css">
</head><body>
<a class="skip" href="#main">Skip to content</a>
<header class="nav light">
  <a class="brand" href="{up}"><img class="logo" src="{up}assets/logo.svg" alt="" width="36" height="36"> mouser</a>
  <button class="menu" aria-expanded="false" aria-controls="links">Menu</button>
  <nav id="links" aria-label="Main">{nav}<a href="https://github.com/WeirdBrains/mouser">GitHub</a></nav>
</header>
<main id="main"><section class="light doc">{note}{body}</section></main>
<footer class="docfoot">Generated from <a href="https://github.com/WeirdBrains/mouser/tree/main/docs">docs/</a> on main. Do not edit by hand.</footer>
<script src="{up}main.js"></script>
</body></html>
"""

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--docs", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--extra", default=str(pathlib.Path(__file__).resolve().parent.parent / "site-src"))
    a = ap.parse_args()
    docs, out, extra = pathlib.Path(a.docs), pathlib.Path(a.out), pathlib.Path(a.extra)
    mapping = tomllib.loads((docs / "sections.toml").read_text())["docs"]
    unmapped = [p for p in docs.rglob("*.md") if "historical" not in p.parts
                and p.relative_to(docs).as_posix() not in mapping]
    if unmapped:
        raise SystemExit(f"unmapped docs: {unmapped}")
    if out.exists():
        shutil.rmtree(out)

    # rel doc path -> (section, slug)
    where = {}
    for rel, sec in mapping.items():
        if sec != "index":
            where[rel] = (sec, rel.removesuffix(".md").replace("/README", "").replace("/", "-").lower())
    for p in sorted((docs / "historical").glob("*.md")):
        where["historical/" + p.name] = ("archive", p.stem.lower())

    def fix_links(text, rel):
        base = posixpath.dirname(rel)
        def sub(m):
            label, target = m.group(1), m.group(2)
            if re.match(r"(https?:|mailto:|#)", target):
                return m.group(0)
            path, _, anchor = target.partition("#")
            anchor = "#" + anchor if anchor else ""
            norm = posixpath.normpath(posixpath.join(base, path)) if path else rel
            if path.endswith("/") or norm in ("docs", "."):
                norm = (norm + "/README.md").lstrip("./") if norm != "." else "README.md"
            if norm in where:
                s, slug = where[norm]
                return f"{label}(../{s}/{slug}.html{anchor})"
            if norm == "README.md":
                return f"{label}(../{anchor})"
            gh = norm[3:] if norm.startswith("../") else "docs/" + norm
            return f"{label}(https://github.com/WeirdBrains/mouser/blob/main/{gh}{anchor})"
        return re.sub(r"(\]\s*)\(([^)\s]+)\)", sub, text)

    md = lambda t: markdown.markdown(t, extensions=["fenced_code", "tables", "toc", "sane_lists"])
    lists = {k: [] for k in list(SECTIONS) + ["archive"]}
    for rel, (sec, slug) in sorted(where.items()):
        text = (docs / rel).read_text()
        title = title_of(text, slug)
        src = f'<p class="docsrc">Source: <a href="https://github.com/WeirdBrains/mouser/blob/main/docs/{rel}">docs/{rel}</a></p>'
        dest = out / sec / f"{slug}.html"
        dest.parent.mkdir(parents=True, exist_ok=True)
        dest.write_text(page(title, md(fix_links(text, rel)) + src, 2, sec if sec in SECTIONS else None))
        lists[sec].append((title, f"{slug}.html", None, rel))
    for p in sorted(extra.rglob("*.md")) if extra.exists() else []:
        sec = p.parent.name
        meta, text = front(p.read_text())
        title = meta.get("title") or title_of(text, p.stem)
        status = meta.get("status", "")
        note = f'<p class="status {status}">{BADGE[status]}: {html.escape(meta.get("note", ""))}</p>' if status in BADGE else ""
        dest = out / sec / f"{p.stem}.html"
        dest.parent.mkdir(parents=True, exist_ok=True)
        dest.write_text(page(title, md(text), 2, sec, note))
        lists[sec].append((title, f"{p.stem}.html", status, None))

    for sec, (name, blurb) in list(SECTIONS.items()) + [("archive", ("Archive", "Finished plans, kept for history."))]:
        items = "".join(
            f'<li><a href="{href}">{html.escape(t)}</a>{f" <span class=badge>{BADGE[s]}</span>" if s in BADGE else ""}</li>'
            for t, href, s, _ in sorted(lists[sec], key=lambda x: (x[2] or "", x[0].lower())))
        body = f"<h1>{name}</h1><p class=lede>{blurb}</p>" + (f"<ul class=doclist>{items}</ul>" if items else "<p>Coming soon.</p>")
        if sec == "developer":
            body += '<p>See also the <a href="../archive/">archive</a> of finished plans.</p>'
        (out / sec).mkdir(parents=True, exist_ok=True)
        (out / sec / "index.html").write_text(page(name, body, 2, sec if sec in SECTIONS else None))
    cards = "".join(f'<li><h3><a href="{k}/">{v[0]}</a></h3><p>{v[1]}</p><p class=count>{len(lists[k])} pages</p></li>' for k, v in SECTIONS.items())
    (out / "index.html").write_text(page("Docs", f"<h1>Mouser docs</h1><ul class=grid>{cards}</ul>", 1))

if __name__ == "__main__":
    main()
