#!/bin/bash
#
# generate-cargo-sources.bash — Vendor Cargo crates for offline Flatpak builds.
#
# Requires git and python3 with aiohttp + tomlkit. On Debian/Ubuntu:
#   sudo apt-get install -y git python3 python3-aiohttp python3-tomlkit

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

GENERATOR_COMMIT="4d5e760321236bd96fc1c6db9ec94c336600c114"
GENERATOR_URL="https://raw.githubusercontent.com/flatpak/flatpak-builder-tools/${GENERATOR_COMMIT}/cargo/flatpak-cargo-generator.py"

CROSVM_URL="https://chromium.googlesource.com/crosvm/crosvm"
CROSVM_COMMIT="a96cb379acf55a75887cbba190666e7d22ff9dbf"

WORKDIR=""
cleanup() { [ -n "$WORKDIR" ] && rm -rf "$WORKDIR"; return 0; }
trap cleanup EXIT
WORKDIR=$(mktemp -d)

GENERATOR="$WORKDIR/flatpak-cargo-generator.py"
curl -fsSL -o "$GENERATOR" "$GENERATOR_URL"

# generate_sources <Cargo.lock path> <output file>
generate_sources() {
    python3 "$GENERATOR" "$1" -o "$2"
    echo "    → $(basename "$2")"
}

echo "==> cargo-sources.json (bubbles app)..."
generate_sources "$SCRIPT_DIR/Cargo.lock" "$SCRIPT_DIR/cargo-sources.json"

echo "==> crosvm-cargo-sources.json (crosvm @ ${CROSVM_COMMIT:0:12}…)..."
git clone --filter=blob:none --quiet "$CROSVM_URL" "$WORKDIR/crosvm"
git -C "$WORKDIR/crosvm" checkout --quiet "$CROSVM_COMMIT" -- Cargo.lock
generate_sources "$WORKDIR/crosvm/Cargo.lock" "$SCRIPT_DIR/crosvm-cargo-sources.json"
