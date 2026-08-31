$ErrorActionPreference = 'Stop'

# Terminate running Hydra processes to prevent file locks during upgrade/uninstall
Get-Process -Name 'hydra', 'hydra-gui' -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue