#!/usr/bin/env bash
# Builds sidescreen-display, the helper that creates Sidescreen's virtual display (macOS only).
# Output: target/sidescreen-display. install-mac.sh puts it inside the app before signing.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
mkdir -p "$ROOT/target"
xcrun swiftc -O -swift-version 5 \
  -import-objc-header "$ROOT/mac/display/CGVirtualDisplay.h" \
  "$ROOT/mac/display/main.swift" \
  -o "$ROOT/target/sidescreen-display"
echo "built: $ROOT/target/sidescreen-display"
