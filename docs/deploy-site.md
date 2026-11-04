# Deploying the website

mouser.weirdbrains.com is a static site in `site/` on the **`site`** branch.

- Pushing `site` runs the Pages workflow (`.github/workflows/pages.yml`), which
  uploads `site/` and deploys it to GitHub Pages. It can also be started by hand
  (workflow_dispatch).
- `site/CNAME` holds `mouser.weirdbrains.com`; DNS is a CNAME to
  `weirdbrains.github.io` (not proxied), and Pages enforces HTTPS.
- `site/fuzzball/` is the Fuzzball crate page; the crate's own spec site is
  https://weirdbrains.github.io/fuzzball/.
- After a release, bump the version and download links in `site/index.html`
  (commit `site: vX.Y.Z`). `main` never deploys the site.
