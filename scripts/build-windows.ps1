# Builds the Windows installer: target/release/bundle/nsis/Sidescreen_<version>_x64-setup.exe
#
# With the updater's signing key it also signs the installer (...-setup.exe.sig) and writes
# latest.json beside it. Upload latest.json with the installer, under exactly that name.
# The key is TAURI_SIGNING_PRIVATE_KEY or TAURI_SIGNING_PRIVATE_KEY_PATH if set, otherwise
# %USERPROFILE%\.sidescreen-signing\sidescreen.key (made with `tauri signer generate`, no
# password). Without a key the installer is built unsigned: it installs by hand, not as an update.
#
# Usage: scripts\build-windows.ps1 [-Notes "release notes for latest.json"]
param([string]$Notes = "")
$ErrorActionPreference = "Stop"
# Keep the builder's home folder (and so their user name) out of the binaries.
$env:RUSTFLAGS = "--remap-path-prefix=$HOME=~"
$env:CI = "true"

$root = Resolve-Path (Join-Path $PSScriptRoot "..")
$app = Join-Path $root "app"
$version = (Get-Content (Join-Path $app "src-tauri\tauri.conf.json") -Raw | ConvertFrom-Json).version
$nsis = Join-Path $root "target\release\bundle\nsis"
$installer = Join-Path $nsis "Sidescreen_${version}_x64-setup.exe"
$manifest = Join-Path $nsis "latest.json"

$keyFile = Join-Path $HOME ".sidescreen-signing\sidescreen.key"
if ($env:TAURI_SIGNING_PRIVATE_KEY_PATH) { $keyFile = $env:TAURI_SIGNING_PRIVATE_KEY_PATH }
$loadedKey = $false
if (-not $env:TAURI_SIGNING_PRIVATE_KEY -and (Test-Path $keyFile)) {
    # For this build only, and never printed.
    $env:TAURI_SIGNING_PRIVATE_KEY = (Get-Content $keyFile -Raw).Trim()
    $loadedKey = $true
}
$sign = [bool]$env:TAURI_SIGNING_PRIVATE_KEY

$buildArgs = @("run", "tauri", "build", "--", "--bundles", "nsis")
$extraConfig = $null
if ($sign) {
    # Updater artifacts stay off in tauri.conf.json, so builds without the key (and the Mac's) work.
    $extraConfig = Join-Path ([IO.Path]::GetTempPath()) "sidescreen-updater-$PID.json"
    Set-Content -Path $extraConfig -Value '{ "bundle": { "createUpdaterArtifacts": true } }' -Encoding ascii
    $buildArgs += @("--config", $extraConfig)
} else {
    Write-Warning "No updater signing key: the installer is not signed for in-place updates and no latest.json is written."
}

Remove-Item -ErrorAction SilentlyContinue $manifest, "$installer.sig"
Push-Location $app
try {
    npm @buildArgs
    if ($LASTEXITCODE -ne 0) { throw "tauri build failed ($LASTEXITCODE)" }
    if ($sign) {
        if (-not (Test-Path "$installer.sig")) { throw "the build wrote no signature: $installer.sig" }
        node scripts/latest-json.mjs $version "$installer.sig" $manifest $Notes
        if ($LASTEXITCODE -ne 0) { throw "latest.json failed ($LASTEXITCODE)" }
    }
} finally {
    Pop-Location
    if ($extraConfig) { Remove-Item -ErrorAction SilentlyContinue $extraConfig }
    if ($loadedKey) { Remove-Item -ErrorAction SilentlyContinue Env:TAURI_SIGNING_PRIVATE_KEY }
}
