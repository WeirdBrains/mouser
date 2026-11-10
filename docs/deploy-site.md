# Deploying the website

mouser.weirdbrains.com is a [Jaspr](https://jaspr.site) static site. The Dart
project lives in `site_jaspr/` on **`main`**. `jaspr build` writes
plain HTML to `site_jaspr/build/jaspr/`, which GitHub Actions deploys to
GitHub Pages.

- The Docs site workflow (`.github/workflows/docs-site.yml`) builds and
  deploys on a pushed `v*` tag, on a push to `main` that touches the site or
  `docs/`, and when started by hand (`workflow_dispatch`). It syncs
  `site/assets`, `site/style.css`, `site/main.js` and `site/CNAME` into
  `site_jaspr/web/`, runs `jaspr build`, and deploys `site_jaspr/build/jaspr`.
  The old `site` branch is no longer used for deploys.
- `site/CNAME` (copied to the Jaspr output) holds `mouser.weirdbrains.com`; DNS
  is a CNAME to `weirdbrains.github.io` (not proxied), and Pages enforces HTTPS.
- The Fuzzball crate page is `site_jaspr/lib/pages/fuzzball.dart` (URL
  `/fuzzball/`); the crate's own spec site is
  https://weirdbrains.github.io/fuzzball/.
- `scripts/release.py` bumps the version and download links in
  `site_jaspr/lib/pages/home.dart` in the same `release: vX.Y.Z` commit as the
  crate version. The version appears in the hero's release
  link and in every install card that downloads a release asset: Arch
  (`mouser-bin-X.Y.Z-1-x86_64.pkg.tar.zst`), Debian/Ubuntu
  (`mouser_X.Y.Z_amd64.deb`), Fedora/RHEL (`mouser-X.Y.Z-1.x86_64.rpm`) and
  Windows (`MouserSetup-X.Y.Z.exe`; the portable zip and macOS names carry no
  version). All of them change together; search `site_jaspr/lib/pages/home.dart`
  for the old version string, replace it, and open each link to confirm the
  asset exists in the release.
- **Screenshots** gallery at `/screenshots/` (`site_jaspr/lib/pages/screenshots.dart`):
  Omarchy shots under `site/assets/screenshots/` (commander, chat, sidebar, theme
  editor, tiled).
- **Mouser is beautiful** is the hero clip on the front page. The files live at
  `site/assets/mouser-is-beautiful.mp4` and `.webm`, with
  `mouser-is-beautiful-poster.png` as the video `poster` attribute (there is no
  sibling poster `<img>` under the video). The `<video>` is `autoplay` `muted`
  `loop` `playsinline`. Re-record on alien-blue (Hyprland, wf-recorder) after
  theme-editor features ship: use an empty workspace (e.g. 9), keep the right
  sidebar fully in frame, and trim any MCP port-in-use toast from the start.
- Code blocks get a Copy button from `site/main.js` (copied into the Jaspr
  output) and scroll sideways when a line is longer than the card (never
  clipped); install cards are one per row.
- **Docs pages** under `/docs/` are generated at build time, never edited by
  hand. Every `docs/*.md` on main is mapped in `docs/sections.toml` to a public
  section (`user`, `developer`, `mcp`, `api`) or to `internal`. Public docs
  become pages; `internal` docs (this page, the release process, HANDOVER) stay
  in the repo only and never appear on the site. `docs/historical/` goes to the
  archive. The Docs site workflow fails if a
  doc is unmapped, then builds the Jaspr site with `MOUSER_DOCS_DIR` pointed at
  `docs/` and `MOUSER_SITE_SRC` at `site-src/`. Pages that don't come from
  `docs/` (the API crate table, drafts, "coming soon" stubs) live in
  `site-src/<section>/*.md`, with `title`/`status` front
  matter. Manual rebuild from a `main` checkout:

  ```bash
  ./site_jaspr/tool/sync_web.sh
  cd site_jaspr
  MOUSER_DOCS_DIR=$PWD/../docs MOUSER_SITE_SRC=$PWD/../site-src jaspr build --sitemap-domain https://mouser.weirdbrains.com
  ```
