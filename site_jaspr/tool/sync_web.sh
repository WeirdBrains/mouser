#!/usr/bin/env bash
# Copy the live site's static files into the Jaspr web/ folder.
# Assets stay in site/assets (not duplicated in git); style.css and main.js
# are copied so a local `jaspr build` matches what CI deploys.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
WEB="$(cd "$(dirname "$0")/../web" && pwd)"
cp "$ROOT/site/style.css" "$WEB/style.css"
cp "$ROOT/site/main.js" "$WEB/main.js"
cp "$ROOT/site/CNAME" "$WEB/CNAME"
rm -rf "$WEB/assets"
cp -a "$ROOT/site/assets" "$WEB/assets"
echo "synced web assets from $ROOT/site into $WEB"
