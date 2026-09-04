$ErrorActionPreference = 'Stop'

$packageArgs = @{
  packageName       = 'hydra-download-manager'
  fileType          = 'exe'
  # NSIS installer (see scripts/windows/hydra-installer.nsi): the silent
  # switch is an uppercase /S. Inno Setup switches like /VERYSILENT are
  # ignored by NSIS, which leaves the wizard on screen and hangs the install.
  # /S selects the installer's defaults: app, IPC host, CLI + PATH, browser
  # extensions, Start-menu and desktop shortcuts. Skip the desktop shortcut
  # with: choco install hydra-download-manager --install-arguments="'/NODESKTOP'"
  silentArgs        = '/S'
  validExitCodes    = @(0)

  # x64 (AMD64 / Intel)
  url64             = 'https://github.com/ja7ad/hydra/releases/download/v0.4.0/hydra-0.4.0-windows-x64-setup.exe'
  checksum64        = '39568C446B54E403874F48F4D442187C3ED1252712D7D7E62C7B95E536CDD59E'
  checksumType64    = 'sha256'

  # ARM64
  url64arm          = 'https://github.com/ja7ad/hydra/releases/download/v0.4.0/hydra-0.4.0-windows-arm64-setup.exe'
  checksum64arm     = 'E50D06B875B825DE35F22BA40ACC84C878E812BB5617E70B23777457A0A56CB7'
  checksumType64arm = 'sha256'
}

Install-ChocolateyPackage @packageArgs