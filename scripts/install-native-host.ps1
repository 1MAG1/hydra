# Copyright (C) 2026 Javad Rajabzadeh
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Install the Hydra native-messaging host on Windows.
#
#   powershell -ExecutionPolicy Bypass -File scripts\install-native-host.ps1
#   ... -NoBuild -HostBin C:\path\to\hydra-host.exe
#
# Windows has no NativeMessagingHosts directory: each browser reads a
# registry value that points at the manifest file. Chromium browsers key it
# by extension origin, Firefox by add-on id, so two manifest files are
# written under %LOCALAPPDATA%\Hydra.
#
# The host is only the FALLBACK transport (and the only thing that can start
# Hydra when it is not running) — day to day the extension talks to the app
# over ws://127.0.0.1:6799, which needs no registration at all.

param(
  [switch]$NoBuild,
  [string]$HostBin = ""
)

$ErrorActionPreference = "Stop"
$Repo = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
$HostName = "com.hydra.host"

if (-not $HostBin) {
  if (-not $NoBuild) {
    Write-Host "building hydra-host + hydra-gui (release)..."
    Push-Location $Repo
    cargo build --release -p hya-host -p hya-gui
    Pop-Location
  }
  $HostBin = Join-Path $Repo "target\release\hydra-host.exe"
}
if (-not (Test-Path $HostBin)) { throw "host binary not found: $HostBin" }
$HostBin = (Resolve-Path $HostBin).Path

# Extension ids: Chromium's is derived from the pinned manifest key, exactly
# as the POSIX installer does, so the two can never drift.
$key = (Get-Content (Join-Path $Repo "extensions\chrome\manifest.json") -Raw |
        ConvertFrom-Json).key
$sha = [System.Security.Cryptography.SHA256]::Create().ComputeHash(
         [Convert]::FromBase64String($key))
$ExtId = -join ($sha[0..15] | ForEach-Object {
  [char](97 + ($_ -shr 4)), [char](97 + ($_ -band 0x0F))
})
$FfId = (Get-Content (Join-Path $Repo "extensions\firefox\manifest.json") -Raw |
         ConvertFrom-Json).browser_specific_settings.gecko.id
Write-Host "chromium extension id: $ExtId"
Write-Host "firefox add-on id:     $FfId"

$OutDir = Join-Path $env:LOCALAPPDATA "Hydra"
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

# JSON needs the path escaped for Windows separators.
$escaped = $HostBin.Replace('\', '\\')
$chromeManifest = Join-Path $OutDir "$HostName.json"
@"
{
  "name": "$HostName",
  "description": "Hydra Download Manager native host",
  "path": "$escaped",
  "type": "stdio",
  "allowed_origins": ["chrome-extension://$ExtId/"]
}
"@ | Set-Content -Encoding UTF8 $chromeManifest

$firefoxManifest = Join-Path $OutDir "$HostName.firefox.json"
@"
{
  "name": "$HostName",
  "description": "Hydra Download Manager native host",
  "path": "$escaped",
  "type": "stdio",
  "allowed_extensions": ["$FfId"]
}
"@ | Set-Content -Encoding UTF8 $firefoxManifest

# HKCU only: a per-user install needs no elevation.
$targets = @(
  @{ Name = "Chrome";   Path = "HKCU:\Software\Google\Chrome\NativeMessagingHosts\$HostName";        Manifest = $chromeManifest },
  @{ Name = "Edge";     Path = "HKCU:\Software\Microsoft\Edge\NativeMessagingHosts\$HostName";       Manifest = $chromeManifest },
  @{ Name = "Brave";    Path = "HKCU:\Software\BraveSoftware\Brave-Browser\NativeMessagingHosts\$HostName"; Manifest = $chromeManifest },
  @{ Name = "Chromium"; Path = "HKCU:\Software\Chromium\NativeMessagingHosts\$HostName";             Manifest = $chromeManifest },
  @{ Name = "Vivaldi";  Path = "HKCU:\Software\Vivaldi\NativeMessagingHosts\$HostName";              Manifest = $chromeManifest },
  @{ Name = "Firefox";  Path = "HKCU:\Software\Mozilla\NativeMessagingHosts\$HostName";              Manifest = $firefoxManifest }
)
foreach ($t in $targets) {
  New-Item -Path $t.Path -Force | Out-Null
  # The key's DEFAULT value holds the manifest path.
  Set-ItemProperty -Path $t.Path -Name "(default)" -Value $t.Manifest
  Write-Host "registered: $($t.Name) -> $($t.Manifest)"
}

Write-Host ""
Write-Host "Load the extension:"
Write-Host "  Chromium family: chrome://extensions -> Developer mode ->"
Write-Host "    Load unpacked -> $Repo\extensions\chrome"
Write-Host "  Firefox: about:debugging#/runtime/this-firefox -> Load Temporary"
Write-Host "    Add-on -> $Repo\extensions\firefox\manifest.json"
Write-Host "(Restart the browser once so it re-reads the registry.)"
