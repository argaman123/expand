# Builds the Windows installer: target/release/bundle/nsis/Sidescreen_<version>_x64-setup.exe
$ErrorActionPreference = "Stop"
# Keep the builder's home folder (and so their user name) out of the binaries.
$env:RUSTFLAGS = "--remap-path-prefix=$HOME=~"
$env:CI = "true"
Push-Location (Join-Path $PSScriptRoot "..\app")
try { npm run tauri build -- --bundles nsis } finally { Pop-Location }
