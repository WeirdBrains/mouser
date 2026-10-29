#!/usr/bin/env bash
set -euo pipefail
# Generate Mouser icons from SVG using the vendored Rust tool (no external installs).
# Outputs:
# - PNGs at sizes: 16,24,32,48,64,128,256,512,1024 under resources/icons/png/
# - Linux hicolor tree under resources/icons/hicolor/<size>x<size>/apps/mouser.png
# - macOS ICNS at resources/macos/mouser.icns
# - Windows ICO at resources/windows/mouser.ico
here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$here"

echo "Building icon-tools..."
cargo build -p icon-tools --release

echo "Generating icons..."
target/release/icon-tools

echo "Done. Notable outputs:"
echo "  SVG (master): assets/icon/mouser.svg"
echo "  PNG 512: resources/icons/png/mouser_512.png"
echo "  ICO: resources/windows/mouser.ico"
echo "  ICNS: resources/macos/mouser.icns"

