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
# Default install is the GUI bundle: hydra.exe, hydra-gui.exe, hydra-host.exe,
# hydra-updater.exe plus the browser extensions and native-host installer, into
# %LOCALAPPDATA%\Programs\Hydra. -Cli installs only hydra.exe.
#
# A GUI install also gets a start-menu shortcut (-Desktop adds one on the
# desktop) and an "Apps & features" entry, so Hydra is listed and uninstallable
# the way Windows expects — the same things the .exe installer registers.

param(
  [switch]$Cli,
  [switch]$Beta,
  [switch]$Desktop,
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

# Assets are named from the workspace manifest version, which may stay on the
# plain release version while the tag carries a pre-release suffix: the
# v0.3.2-rc release ships hydra-0.3.2-* files. Try the tag spelling first, then
# the tag without its suffix.
$Core = $Ver -replace '-.*$', ''
$AssetPrefix = if ($Cli) { "hydra-cli" } else { "hydra" }
$Candidates = if ($Core -ne $Ver) { @($Ver, $Core) } else { @($Ver) }
$DlBase = "https://github.com/$Repo/releases/download/$Version"

$Mode = if ($Cli) { "cli" } else { "gui" }
Write-Host "hydra $Version ($Mode) -> $InstallDir  [windows/$Arch]"

$Tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("hydra-install-" + [Guid]::NewGuid())
New-Item -ItemType Directory -Force -Path $Tmp | Out-Null
try {
  $Name = $null
  $Zip = $null
  foreach ($Cand in $Candidates) {
    $Try = "$AssetPrefix-$Cand-windows-$Arch"
    $Dest = Join-Path $Tmp "$Try.zip"
    Write-Host "downloading $DlBase/$Try.zip"
    try {
      Invoke-WebRequest -UseBasicParsing -Uri "$DlBase/$Try.zip" -OutFile $Dest
      $Name = $Try; $Zip = $Dest; break
    } catch {
      Remove-Item $Dest -Force -ErrorAction SilentlyContinue
      # Missing the first spelling is expected on a pre-release tag; only the
      # last candidate's failure is fatal.
      if ($Cand -eq $Candidates[-1]) { throw }
    }
  }
  Expand-Archive -Path $Zip -DestinationPath $Tmp
  $Src = Join-Path $Tmp $Name

  New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null

  # A running Hydra keeps its .exe files locked and Copy-Item would fail
  # halfway through — this script is the upgrade path as much as the first
  # install. Ask the GUI to close before forcing anything, so it gets to
  # persist its state.
  $Running = @(Get-Process -Name hydra-gui, hydra-host, hydra-updater -ErrorAction SilentlyContinue)
  if ($Running.Count -gt 0) {
    Write-Host "stopping the running Hydra..."
    foreach ($p in $Running) { $null = $p.CloseMainWindow() }
    Start-Sleep -Milliseconds 1200
    $Running | Where-Object { -not $_.HasExited } | Stop-Process -Force -ErrorAction SilentlyContinue
  }

  Copy-Item (Join-Path $Src "hydra.exe") $InstallDir -Force
  Write-Host "installed $InstallDir\hydra.exe"

  if (-not $Cli) {
    Copy-Item (Join-Path $Src "hydra-gui.exe")  $InstallDir -Force
    Copy-Item (Join-Path $Src "hydra-host.exe") $InstallDir -Force
    Write-Host "installed $InstallDir\hydra-gui.exe"
    Write-Host "installed $InstallDir\hydra-host.exe"
    # The self-update finisher. The GUI runs it from a temp copy, but keeping
    # one here means the in-app update refreshes it like any other file:
    # hya_updater::apply only replaces names that already exist.
    $UpdaterExe = Join-Path $Src "hydra-updater.exe"
    if (Test-Path $UpdaterExe) {
      Copy-Item $UpdaterExe $InstallDir -Force
      Write-Host "installed $InstallDir\hydra-updater.exe"
    }

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

    # Shortcuts. The GUI exe embeds hydra.ico and a VERSIONINFO block
    # (crates/hydra-gui/build.rs), so the icon and the product name come along
    # for free — IconLocation just makes the source explicit.
    $GuiExe = Join-Path $InstallDir "hydra-gui.exe"
    $Shell = New-Object -ComObject WScript.Shell
    function New-HydraShortcut([string]$LnkPath) {
      $Lnk = $Shell.CreateShortcut($LnkPath)
      $Lnk.TargetPath = $GuiExe
      $Lnk.WorkingDirectory = $InstallDir
      $Lnk.Description = "Multi-connection download accelerator"
      $Lnk.IconLocation = "$GuiExe,0"
      $Lnk.Save()
    }
    $StartMenu = Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs"
    New-HydraShortcut (Join-Path $StartMenu "Hydra Download Manager.lnk")
    Write-Host "installed start-menu shortcut"
    if ($Desktop) {
      New-HydraShortcut (Join-Path ([Environment]::GetFolderPath("Desktop")) "Hydra Download Manager.lnk")
      Write-Host "installed desktop shortcut"
    }

    # "Apps & features" entry, so Hydra is listed and uninstallable from
    # Settings like any other app. Its own key, NOT the .exe installer's
    # (...\Uninstall\Hydra): that is a different install in a different
    # directory with its own uninstaller, and overwriting its UninstallString
    # would orphan it.
    $UninstallScript = Join-Path $InstallDir "scripts\uninstall.ps1"
    if (-not (Test-Path $UninstallScript)) {
      # Archives before this change do not carry it; fetch the copy that
      # matches this release rather than whatever main happens to hold.
      try {
        Invoke-WebRequest -UseBasicParsing `
          -Uri "https://raw.githubusercontent.com/$Repo/$Version/uninstall.ps1" `
          -OutFile $UninstallScript
      } catch {
        $UninstallScript = $null
      }
    }
    $UninstallCmd = if ($UninstallScript) {
      "powershell -NoProfile -ExecutionPolicy Bypass -File `"$UninstallScript`" -InstallDir `"$InstallDir`""
    } else {
      # No local copy: fetch at removal time. `irm | iex` cannot take
      # parameters, so this uses the scriptblock form — otherwise a custom
      # -InstallDir would be uninstalled from the default location instead.
      "powershell -NoProfile -ExecutionPolicy Bypass -Command `"& ([scriptblock]::Create((irm https://raw.githubusercontent.com/$Repo/main/uninstall.ps1))) -InstallDir '$InstallDir'`""
    }
    $UninstKey = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\HydraDownloadManager"
    New-Item -Path $UninstKey -Force | Out-Null
    $SizeKb = [int](((Get-ChildItem $InstallDir -Recurse -File -ErrorAction SilentlyContinue |
      Measure-Object -Property Length -Sum).Sum) / 1KB)
    $Props = [ordered]@{
      DisplayName     = "Hydra Download Manager"
      DisplayVersion  = $Ver
      Publisher       = "Javad Rajabzadeh"
      URLInfoAbout    = "https://github.com/$Repo"
      DisplayIcon     = "$GuiExe,0"
      InstallLocation = $InstallDir
      UninstallString = $UninstallCmd
      NoModify        = 1
      NoRepair        = 1
      EstimatedSize   = $SizeKb
    }
    foreach ($k in $Props.Keys) {
      $type = if ($Props[$k] -is [int]) { "DWord" } else { "String" }
      New-ItemProperty -Path $UninstKey -Name $k -Value $Props[$k] -PropertyType $type -Force | Out-Null
    }
    Write-Host "registered Hydra in Apps & features"

    # A copy installed by the .exe installer is a separate install with its own
    # uninstaller; say so rather than letting two entries confuse later.
    if (Test-Path "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\Hydra") {
      Write-Warning "Hydra is also installed by the .exe installer; uninstall that copy from Apps & features to avoid running two installs."
    }
  }

  # Put the install dir on the user PATH so `hydra` works in new shells.
  $UserPath = [Environment]::GetEnvironmentVariable("Path", "User")
  if (($UserPath -split ";") -notcontains $InstallDir) {
    # A user with no PATH of their own must not get a leading ";".
    $NewPath = if ($UserPath) { "$UserPath;$InstallDir" } else { $InstallDir }
    # .NET broadcasts WM_SETTINGCHANGE for a User-scope write, so newly
    # started programs pick this up without a sign-out.
    [Environment]::SetEnvironmentVariable("Path", $NewPath, "User")
    Write-Host "added $InstallDir to your user PATH (open a new terminal to pick it up)"
  }

  Write-Host "done."
}
finally {
  Remove-Item $Tmp -Recurse -Force -ErrorAction SilentlyContinue
}
