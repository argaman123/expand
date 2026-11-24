#!/usr/bin/env bash
# Builds the Mac release: Sidescreen.app with its display and panel helpers inside, packed in a DMG.
# Output: target/release/bundle/dmg/Sidescreen_<version>_aarch64.dmg
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
export PATH="$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin:$HOME/.cargo/bin:/opt/homebrew/bin:$PATH"

# Keep the builder's home folder (and so their user name) out of the binaries.
# -Wl,-S: the linker leaves out the debug map, which lists object files by full path.
export RUSTFLAGS="--remap-path-prefix=$HOME=~ -C link-arg=-Wl,-S"

bash "$ROOT/scripts/build-display-helper.sh"
bash "$ROOT/scripts/build-panel-helper.sh"
# CI=true: skip the Finder window styling, which needs a logged-in Finder session.
(cd "$ROOT/app" && CI=true npm run tauri build -- --bundles app,dmg)
ls -1 "$ROOT"/target/release/bundle/dmg/*.dmg
