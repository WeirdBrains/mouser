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
  (commit `site: vX.Y.Z`). `main` never deploys the site. The version appears in
  the hero's release link and in every install card that downloads a release
  asset: Arch (`mouser-bin-X.Y.Z-1-x86_64.pkg.tar.zst`), Debian/Ubuntu
  (`mouser_X.Y.Z_amd64.deb`), Fedora/RHEL (`mouser-X.Y.Z-1.x86_64.rpm`) and
  Windows (`MouserSetup-X.Y.Z.exe`; the portable zip and macOS names carry no
  version). All of them change together, for example
  `sed -i 's/1\.3\.0/1.4.0/g' site/index.html`; check with
  `grep -n '1\.3\.0' site/index.html` first that only versions match, and open each
  link to confirm the asset exists in the release.
- Code blocks get a Copy button from `site/main.js` and scroll sideways when a
  line is longer than the card (never clipped); install cards are one per row.
