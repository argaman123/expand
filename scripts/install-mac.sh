#!/usr/bin/env bash
# Builds Expand on the Mac, replaces /Applications/Expand.app, signs it with the
# private identity (so macOS keeps its permissions) and starts it. Run from anywhere.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
export PATH="$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin:/opt/homebrew/bin:$PATH"

# The helpers first: the bundle copies them into Contents/MacOS (tauri.conf.json).
bash "$ROOT/scripts/build-display-helper.sh"
bash "$ROOT/scripts/build-panel-helper.sh"
(cd "$ROOT/app" && npm run tauri build -- --bundles app)
cp "$ROOT/target/expand-display" "$ROOT/target/expand-panel"   "$ROOT/target/release/bundle/macos/Expand.app/Contents/MacOS/"

osascript -e 'tell application "Expand" to quit' 2>/dev/null || true
sleep 3
rm -rf /Applications/Expand.app
cp -R "$ROOT/target/release/bundle/macos/Expand.app" /Applications/
bash "$ROOT/scripts/sign-mac.sh" /Applications/Expand.app
open -a Expand
