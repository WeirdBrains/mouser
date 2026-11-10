# Mouser website (Jaspr)

The Mouser site at [mouser.weirdbrains.com](https://mouser.weirdbrains.com) is a
[Jaspr](https://jaspr.site) project in **static** mode. `jaspr build` pre-renders
plain HTML into `build/jaspr/`, which GitHub Actions deploys from `main`
(see `.github/workflows/docs-site.yml`).

- Hand-written pages (home, `/fuzzball/`, `/screenshots/`) live in `lib/pages/`
  and reuse `web/style.css` (the same stylesheet as the previous static site).
- Docs pages are generated at build time from `docs/` (and `site-src/` extras)
  using the same section map as `scripts/build_docs.py`. Point
  `MOUSER_DOCS_DIR` at `docs/` (the same tree) so the published guides stay in sync.
- Do not add a sibling `.beautiful-poster` image under the hero video.

## Build

From the repository root (never from `~/Projects` on the build machine):

```bash
./site_jaspr/tool/sync_web.sh
cd site_jaspr
jaspr build --sitemap-domain https://mouser.weirdbrains.com
```

Then clear `build/` and `.dart_tool/` when you are done inspecting the output.
