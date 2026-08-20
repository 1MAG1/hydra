# Copyright (C) 2026 Javad Rajabzadeh
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Install the latest Hydra release on Windows.
#
#   powershell -ExecutionPolicy Bypass -File install.ps1          # GUI bundle (default)
#   powershell -ExecutionPolicy Bypass -File install.ps1 -Cli     # CLI only
#   powershell -ExecutionPolicy Bypass -File install.ps1 -Version v0.2.0
#   powershell -ExecutionPolicy Bypass -File install.ps1 -Beta    # newest -rc pre-release when ahead of latest
#
# Or straight from the repo:
#   irm https://raw.githubusercontent.com/ja7ad/hydra/main/install.ps1 | iex
#
# Default install is the GUI bundle: hydra.exe, hydra-gui.exe, hydra-host.exe
# plus the browser extensions and native-host installer, into
# %LOCALAPPDATA%\Programs\Hydra. -Cli installs only hydra.exe.

param(
  [switch]$Cli,
  [switch]$Beta,
  [string]$Version = "",
  [string]$InstallDir = ""
)

$ErrorActionPreference = "Stop"
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

$Repo = "ja7ad/hydra"
if (-not $InstallDir) { $InstallDir = Join-Path $env:LOCALAPPDATA "Programs\Hydra" }

switch ($env:PROCESSOR_ARCHITECTURE) {
  "AMD64" { $Arch = "amd64" }
  "ARM64" { $Arch = "arm64" }
  default { throw "unsupported architecture: $env:PROCESSOR_ARCHITECTURE" }
}

function Get-CoreVersion([string]$Tag) {
  # v0.3.0-rc1 -> [version]0.3.0 (the numeric core decides channel order)
  [version](($Tag.TrimStart("v") -split "[-+]")[0])
}

if (-not $Version) {
  if ($Beta) {
    # The newest -rc pre-release wins only while its version is ahead of the
    # stable release; once stable catches up, -Beta installs stable.
    $Releases = Invoke-RestMethod -UseBasicParsing `
      -Uri "https://api.github.com/repos/$Repo/releases?per_page=30"
    $Stable = ($Releases | Where-Object { -not $_.prerelease } | Select-Object -First 1).tag_name
    $Rc = ($Releases | Where-Object { $_.prerelease -or $_.tag_name -match "-rc" } |
      Select-Object -First 1).tag_name
    if (-not $Stable -and -not $Rc) { throw "could not resolve a release tag" }
    if ($Rc -and (-not $Stable -or ((Get-CoreVersion $Rc) -gt (Get-CoreVersion $Stable)))) {
      $Version = $Rc
    } else {
      $Version = $Stable
    }
  } else {
    $Version = (Invoke-RestMethod -UseBasicParsing `
      -Uri "https://api.github.com/repos/$Repo/releases/latest").tag_name
    if (-not $Version) { throw "could not resolve the latest release tag" }
  }
}
$Ver = $Version.TrimStart("v")

if ($Cli) { $Name = "hydra-cli-$Ver-windows-$Arch" }
else      { $Name = "hydra-$Ver-windows-$Arch" }
$Url = "https://github.com/$Repo/releases/download/$Version/$Name.zip"

$Mode = if ($Cli) { "cli" } else { "gui" }
Write-Host "hydra $Version ($Mode) -> $InstallDir  [windows/$Arch]"

$Tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("hydra-install-" + [Guid]::NewGuid())
New-Item -ItemType Directory -Force -Path $Tmp | Out-Null
try {
  $Zip = Join-Path $Tmp "$Name.zip"
  Write-Host "downloading $Url"
  Invoke-WebRequest -UseBasicParsing -Uri $Url -OutFile $Zip
  Expand-Archive -Path $Zip -DestinationPath $Tmp
  $Src = Join-Path $Tmp $Name

  New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
  Copy-Item (Join-Path $Src "hydra.exe") $InstallDir -Force
  Write-Host "installed $InstallDir\hydra.exe"

  if (-not $Cli) {
    Copy-Item (Join-Path $Src "hydra-gui.exe")  $InstallDir -Force
    Copy-Item (Join-Path $Src "hydra-host.exe") $InstallDir -Force
    Write-Host "installed $InstallDir\hydra-gui.exe"
    Write-Host "installed $InstallDir\hydra-host.exe"

    # Extensions + native-host installer keep the bundle layout (the script
    # resolves the bundle root as the parent of its own directory).
    foreach ($d in "extensions", "scripts") {
      $dest = Join-Path $InstallDir $d
      if (Test-Path $dest) { Remove-Item $dest -Recurse -Force }
      Copy-Item (Join-Path $Src $d) $InstallDir -Recurse -Force
    }
    Write-Host "installed $InstallDir\extensions (browser extensions)"

    # Register the native-messaging host (per-user registry + manifests).
    & (Join-Path $InstallDir "scripts\install-native-host.ps1") `
        -NoBuild -HostBin (Join-Path $InstallDir "hydra-host.exe")

    # Start-menu shortcut for the GUI.
    $StartMenu = Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs"
    $Shell = New-Object -ComObject WScript.Shell
    $Lnk = $Shell.CreateShortcut((Join-Path $StartMenu "Hydra Download Manager.lnk"))
    $Lnk.TargetPath = Join-Path $InstallDir "hydra-gui.exe"
    $Lnk.WorkingDirectory = $InstallDir
    $Lnk.Save()
    Write-Host "installed start-menu shortcut"
  }

  # Put the install dir on the user PATH so `hydra` works in new shells.
  $UserPath = [Environment]::GetEnvironmentVariable("Path", "User")
  if (($UserPath -split ";") -notcontains $InstallDir) {
    [Environment]::SetEnvironmentVariable("Path", "$UserPath;$InstallDir", "User")
    Write-Host "added $InstallDir to your user PATH (open a new terminal to pick it up)"
  }

  Write-Host "done."
}
finally {
  Remove-Item $Tmp -Recurse -Force -ErrorAction SilentlyContinue
}
