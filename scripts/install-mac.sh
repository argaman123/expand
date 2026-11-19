#!/usr/bin/env bash
# Builds Sidescreen on the Mac, replaces /Applications/Sidescreen.app, signs it with the
# private identity (so macOS keeps its permissions) and starts it. Run from anywhere.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
export PATH="$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin:/opt/homebrew/bin:$PATH"

(cd "$ROOT/app" && npm run tauri build -- --bundles app)
bash "$ROOT/scripts/build-display-helper.sh"
cp "$ROOT/target/sidescreen-display" "$ROOT/target/release/bundle/macos/Sidescreen.app/Contents/MacOS/"

osascript -e 'tell application "Sidescreen" to quit' 2>/dev/null || true
sleep 3
rm -rf /Applications/Sidescreen.app
cp -R "$ROOT/target/release/bundle/macos/Sidescreen.app" /Applications/
bash "$ROOT/scripts/sign-mac.sh" /Applications/Sidescreen.app
open -a Sidescreen
