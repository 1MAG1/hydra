; Copyright (C) 2026 Javad Rajabzadeh
; SPDX-License-Identifier: GPL-3.0-or-later
;
; NSIS installer for Hydra Download Manager on Windows.
;
; Compile from THIS directory (paths below are relative to it):
;
;   makensis -DARCH=x64   hydra-installer.nsi
;   makensis -DARCH=arm64 hydra-installer.nsi
;   makensis -DVERSION=0.3.0 -DBUILD_DIR=..\..\target\custom\release hydra-installer.nsi
;
; Expects hydra-gui.exe, hydra-host.exe, and hydra.exe already built in
; BUILD_DIR (default: target\<rust-triple>\release for the chosen ARCH).
; Cross-build from macOS with scripts/build-windows-installer.sh, or
; natively with `cargo build --release -p hya-gui -p hya-host -p hya-cli`.
;
; Installs per-user (no elevation), mirroring scripts/install-native-host.ps1:
;   * Hydra Download Manager  - the GUI app + logo + shortcuts
;   * IPC host                - hydra-host.exe, native-messaging manifests,
;                               and the HKCU registry keys the browsers read
;   * CLI (hydra.exe)         - with $INSTDIR appended to the user PATH so
;                               all binaries resolve in cmd/PowerShell
;   * Browser extensions      - unpacked chrome/ + firefox/ extension sources
;
; Windows has no NativeMessagingHosts directory: each browser reads a
; registry value pointing at a manifest file. Chromium browsers key it by
; extension origin, Firefox by add-on id, so two manifests are written. The
; host is only the FALLBACK transport (and the only thing that can start
; Hydra when it is not running) - day to day the extension talks to the app
; over ws://127.0.0.1:6799, which needs no registration at all.

Unicode true
SetCompressor /SOLID lzma

;--------------------------------
; Defines

!define APP_NAME     "Hydra Download Manager"
!define PUBLISHER    "Javad Rajabzadeh"
!define HOMEPAGE     "https://github.com/ja7ad/hydra"
!define HOST_NAME    "com.hydra.host"
; Chromium id is derived from the pinned key in extensions/chrome/manifest.json
; (first 16 bytes of SHA-256 of the decoded key, mapped a-p), so it can never
; drift as long as the key stays pinned. Recompute if the key ever changes:
;   python3 -c "import json,hashlib,base64;k=json.load(open('extensions/chrome/manifest.json'))['key'];h=hashlib.sha256(base64.b64decode(k)).digest()[:16];print(''.join(chr(97+(b>>4))+chr(97+(b&15)) for b in h))"
!define CHROME_EXT_ID  "jpnonmbbkjdpeebdhkjoliklfhkdcomj"
!define FIREFOX_EXT_ID "hydra@ja7ad.github.io"

!ifndef VERSION
  ; Falls back to the workspace product version ([workspace.package] in
  ; ../../Cargo.toml) so this never drifts out of sync when no -DVERSION is
  ; passed on the command line. Same grep+cut extraction as
  ; .github/workflows/release.yml; piped through a tempfile (rather than
  ; !searchparse /file) because Cargo.toml's UTF-8 comments (em dashes) trip
  ; NSIS's "Bad text encoding" check when read directly, and NSIS strips
  ; backslashes from !system strings so a sed capture-group regex can't be
  ; used inline.
  !tempfile VERSIONFILE
  !system `grep -m1 "^version = " "../../Cargo.toml" | cut -d'"' -f2 > "${VERSIONFILE}"`
  !define /file VERSION "${VERSIONFILE}"
  !delfile "${VERSIONFILE}"
!endif
; VIProductVersion accepts only x.x.x.x numerics, so a pre-release VERSION
; (0.3.0-rc1) passes its numeric part separately; display strings keep the
; full version.
!ifndef NUM_VERSION
  !define NUM_VERSION "${VERSION}"
!endif
!ifndef ARCH
  !define ARCH "x64"
!endif
; "amd64" (or anything else non-arm64) maps to the x86_64 target below.
!if "${ARCH}" == "arm64"
  !define RUST_TARGET "aarch64-pc-windows-msvc"
!else
  !define RUST_TARGET "x86_64-pc-windows-msvc"
!endif
!ifndef BUILD_DIR
  !define BUILD_DIR "..\..\target\${RUST_TARGET}\release"
!endif

!define UNINST_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\Hydra"

Name "${APP_NAME}"
OutFile "..\..\target\hydra-${VERSION}-windows-${ARCH}-setup.exe"

; Per-user install: needs no elevation, and matches the HKCU-only
; native-messaging registration (a per-machine install would need HKLM keys
; for every browser AND still only cover one user's extension profile).
RequestExecutionLevel user
InstallDir "$LOCALAPPDATA\Programs\Hydra"
InstallDirRegKey HKCU "${UNINST_KEY}" "InstallLocation"

VIProductVersion "${NUM_VERSION}.0"
VIAddVersionKey "ProductName"     "${APP_NAME}"
VIAddVersionKey "ProductVersion"  "${VERSION}"
VIAddVersionKey "FileVersion"     "${VERSION}"
VIAddVersionKey "FileDescription" "${APP_NAME} Setup"
VIAddVersionKey "CompanyName"     "${PUBLISHER}"
VIAddVersionKey "LegalCopyright"  "(C) 2026 ${PUBLISHER}. GPL-3.0-or-later."

;--------------------------------
; Includes

!include "MUI2.nsh"
!include "FileFunc.nsh"
!include "WinMessages.nsh"
!include "StrFunc.nsh"
${Using:StrFunc} StrRep
${Using:StrFunc} StrStr
${Using:StrFunc} UnStrRep

;--------------------------------
; UI

!define MUI_ICON   "hydra.ico"
!define MUI_UNICON "hydra.ico"
!define MUI_ABORTWARNING
!define MUI_FINISHPAGE_RUN "$INSTDIR\hydra-gui.exe"
!define MUI_FINISHPAGE_RUN_TEXT "Launch ${APP_NAME}"
!define MUI_FINISHPAGE_SHOWREADME "$INSTDIR\extensions\LOAD-EXTENSIONS.txt"
!define MUI_FINISHPAGE_SHOWREADME_TEXT "How to load the browser extension"
!define MUI_FINISHPAGE_SHOWREADME_NOTCHECKED

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_LICENSE "..\..\LICENSE"
!insertmacro MUI_PAGE_COMPONENTS
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

!insertmacro MUI_LANGUAGE "English"

;--------------------------------
; Sections

Section "Hydra Download Manager" SEC_GUI
  SectionIn RO  ; the app itself is not optional

  ; Replacing a running exe fails with "file in use"; the GUI holds a
  ; single-instance guard anyway, so stop it (and the host) first.
  nsExec::Exec 'taskkill /F /IM hydra-gui.exe'
  nsExec::Exec 'taskkill /F /IM hydra-host.exe'
  Pop $0

  SetOutPath "$INSTDIR"
  File "${BUILD_DIR}\hydra-gui.exe"
  File "hydra.ico"
  File "..\..\LICENSE"

  ; Start-menu shortcut. hydra-gui.exe embeds hydra.ico + VERSIONINFO
  ; (crates/hydra-gui/build.rs), so shortcuts take the exe's own icon.
  CreateDirectory "$SMPROGRAMS\Hydra"
  CreateShortcut "$SMPROGRAMS\Hydra\${APP_NAME}.lnk" "$INSTDIR\hydra-gui.exe"
  CreateShortcut "$SMPROGRAMS\Hydra\Uninstall ${APP_NAME}.lnk" \
    "$INSTDIR\uninstall.exe"

  WriteUninstaller "$INSTDIR\uninstall.exe"

  ; Add/Remove Programs entry (HKCU to match the per-user install).
  WriteRegStr   HKCU "${UNINST_KEY}" "DisplayName"     "${APP_NAME}"
  WriteRegStr   HKCU "${UNINST_KEY}" "DisplayVersion"  "${VERSION}"
  WriteRegStr   HKCU "${UNINST_KEY}" "Publisher"       "${PUBLISHER}"
  WriteRegStr   HKCU "${UNINST_KEY}" "URLInfoAbout"    "${HOMEPAGE}"
  WriteRegStr   HKCU "${UNINST_KEY}" "DisplayIcon"     "$INSTDIR\hydra.ico"
  WriteRegStr   HKCU "${UNINST_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr   HKCU "${UNINST_KEY}" "UninstallString" "$\"$INSTDIR\uninstall.exe$\""
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoRepair" 1
  ${GetSize} "$INSTDIR" "/S=0K" $0 $1 $2
  WriteRegDWORD HKCU "${UNINST_KEY}" "EstimatedSize" $0
SectionEnd

Section "Browser IPC Host" SEC_HOST
  SetOutPath "$INSTDIR"
  File "${BUILD_DIR}\hydra-host.exe"

  ; JSON needs the path escaped for Windows separators.
  ${StrRep} $1 "$INSTDIR\hydra-host.exe" "\" "\\"

  ; Chromium-family manifest (keyed by extension origin).
  FileOpen $0 "$INSTDIR\${HOST_NAME}.json" w
  FileWrite $0 '{$\r$\n'
  FileWrite $0 '  "name": "${HOST_NAME}",$\r$\n'
  FileWrite $0 '  "description": "Hydra Download Manager native host",$\r$\n'
  FileWrite $0 '  "path": "$1",$\r$\n'
  FileWrite $0 '  "type": "stdio",$\r$\n'
  FileWrite $0 '  "allowed_origins": ["chrome-extension://${CHROME_EXT_ID}/"]$\r$\n'
  FileWrite $0 '}$\r$\n'
  FileClose $0

  ; Firefox manifest (keyed by add-on id).
  FileOpen $0 "$INSTDIR\${HOST_NAME}.firefox.json" w
  FileWrite $0 '{$\r$\n'
  FileWrite $0 '  "name": "${HOST_NAME}",$\r$\n'
  FileWrite $0 '  "description": "Hydra Download Manager native host",$\r$\n'
  FileWrite $0 '  "path": "$1",$\r$\n'
  FileWrite $0 '  "type": "stdio",$\r$\n'
  FileWrite $0 '  "allowed_extensions": ["${FIREFOX_EXT_ID}"]$\r$\n'
  FileWrite $0 '}$\r$\n'
  FileClose $0

  ; Each browser reads the DEFAULT value of its NativeMessagingHosts key.
  WriteRegStr HKCU "Software\Google\Chrome\NativeMessagingHosts\${HOST_NAME}"              "" "$INSTDIR\${HOST_NAME}.json"
  WriteRegStr HKCU "Software\Microsoft\Edge\NativeMessagingHosts\${HOST_NAME}"             "" "$INSTDIR\${HOST_NAME}.json"
  WriteRegStr HKCU "Software\BraveSoftware\Brave-Browser\NativeMessagingHosts\${HOST_NAME}" "" "$INSTDIR\${HOST_NAME}.json"
  WriteRegStr HKCU "Software\Chromium\NativeMessagingHosts\${HOST_NAME}"                   "" "$INSTDIR\${HOST_NAME}.json"
  WriteRegStr HKCU "Software\Vivaldi\NativeMessagingHosts\${HOST_NAME}"                    "" "$INSTDIR\${HOST_NAME}.json"
  WriteRegStr HKCU "Software\Mozilla\NativeMessagingHosts\${HOST_NAME}"                    "" "$INSTDIR\${HOST_NAME}.firefox.json"
SectionEnd

Section "Command-Line Tool (hydra) + PATH" SEC_CLI
  SetOutPath "$INSTDIR"
  File "${BUILD_DIR}\hydra.exe"

  ; Append $INSTDIR to the per-user PATH (HKCU\Environment) so `hydra`,
  ; `hydra-gui`, and `hydra-host` resolve in cmd, PowerShell, and any
  ; terminal. REG_EXPAND_SZ so pre-existing %VAR% entries keep expanding.
  ReadRegStr $0 HKCU "Environment" "Path"
  ${StrStr} $1 "$0" "$INSTDIR"
  StrCmp $1 "" 0 path_done          ; already present -> skip
  StrCmp $0 "" 0 path_append
  StrCpy $0 "$INSTDIR"              ; PATH was empty/absent
  Goto path_write
path_append:
  StrCpy $0 "$0;$INSTDIR"
path_write:
  WriteRegExpandStr HKCU "Environment" "Path" $0
  ; Tell running shells/Explorer the environment changed; new terminals
  ; pick it up immediately, already-open ones still need a restart.
  SendMessage ${HWND_BROADCAST} ${WM_WININICHANGE} 0 "STR:Environment" /TIMEOUT=5000
path_done:
SectionEnd

Section "Browser Extensions" SEC_EXT
  SetOutPath "$INSTDIR\extensions"
  File /r /x ".gitignore" "..\..\extensions\chrome"
  File /r /x ".gitignore" "..\..\extensions\firefox"

  FileOpen $0 "$INSTDIR\extensions\LOAD-EXTENSIONS.txt" w
  FileWrite $0 'Loading the Hydra browser extension$\r$\n'
  FileWrite $0 '===================================$\r$\n$\r$\n'
  FileWrite $0 'Chromium family (Chrome, Edge, Brave, Vivaldi):$\r$\n'
  FileWrite $0 '  1. Open chrome://extensions (edge://extensions, brave://extensions, ...)$\r$\n'
  FileWrite $0 '  2. Enable "Developer mode"$\r$\n'
  FileWrite $0 '  3. "Load unpacked" -> $INSTDIR\extensions\chrome$\r$\n$\r$\n'
  FileWrite $0 'Firefox:$\r$\n'
  FileWrite $0 '  1. Open about:debugging#/runtime/this-firefox$\r$\n'
  FileWrite $0 '  2. "Load Temporary Add-on" -> $INSTDIR\extensions\firefox\manifest.json$\r$\n$\r$\n'
  FileWrite $0 'Restart the browser once so it re-reads the native-messaging registry.$\r$\n'
  FileClose $0
SectionEnd

Section "Logo & Branding Assets" SEC_LOGO
  SetOutPath "$INSTDIR\assets"
  File "..\..\docs\logo.png"
SectionEnd

Section /o "Desktop Shortcut" SEC_DESKTOP
  CreateShortcut "$DESKTOP\${APP_NAME}.lnk" "$INSTDIR\hydra-gui.exe"
SectionEnd

;--------------------------------
; Component descriptions

!insertmacro MUI_FUNCTION_DESCRIPTION_BEGIN
  !insertmacro MUI_DESCRIPTION_TEXT ${SEC_GUI}     "The Hydra Download Manager application (required)."
  !insertmacro MUI_DESCRIPTION_TEXT ${SEC_HOST}    "Native-messaging host bridging browser extensions to Hydra, plus its registry registration for Chrome, Edge, Brave, Chromium, Vivaldi, and Firefox."
  !insertmacro MUI_DESCRIPTION_TEXT ${SEC_CLI}     "The hydra command-line downloader, with the install directory added to your PATH for cmd, PowerShell, and other terminals."
  !insertmacro MUI_DESCRIPTION_TEXT ${SEC_EXT}     "Unpacked browser-extension sources for Chromium browsers and Firefox (loaded via developer mode)."
  !insertmacro MUI_DESCRIPTION_TEXT ${SEC_LOGO}    "Hydra logo image installed alongside the application."
  !insertmacro MUI_DESCRIPTION_TEXT ${SEC_DESKTOP} "Shortcut to Hydra Download Manager on the desktop."
!insertmacro MUI_FUNCTION_DESCRIPTION_END

;--------------------------------
; Uninstaller

Section "Uninstall"
  nsExec::Exec 'taskkill /F /IM hydra-gui.exe'
  nsExec::Exec 'taskkill /F /IM hydra-host.exe'
  Pop $0

  ; Native-messaging registration.
  DeleteRegKey HKCU "Software\Google\Chrome\NativeMessagingHosts\${HOST_NAME}"
  DeleteRegKey HKCU "Software\Microsoft\Edge\NativeMessagingHosts\${HOST_NAME}"
  DeleteRegKey HKCU "Software\BraveSoftware\Brave-Browser\NativeMessagingHosts\${HOST_NAME}"
  DeleteRegKey HKCU "Software\Chromium\NativeMessagingHosts\${HOST_NAME}"
  DeleteRegKey HKCU "Software\Vivaldi\NativeMessagingHosts\${HOST_NAME}"
  DeleteRegKey HKCU "Software\Mozilla\NativeMessagingHosts\${HOST_NAME}"

  ; Strip $INSTDIR out of the per-user PATH (all three separator shapes).
  ReadRegStr $0 HKCU "Environment" "Path"
  ${UnStrRep} $0 "$0" ";$INSTDIR" ""
  ${UnStrRep} $0 "$0" "$INSTDIR;" ""
  ${UnStrRep} $0 "$0" "$INSTDIR"  ""
  WriteRegExpandStr HKCU "Environment" "Path" $0
  SendMessage ${HWND_BROADCAST} ${WM_WININICHANGE} 0 "STR:Environment" /TIMEOUT=5000

  Delete "$INSTDIR\hydra-gui.exe"
  Delete "$INSTDIR\hydra.exe"
  Delete "$INSTDIR\hydra-host.exe"
  Delete "$INSTDIR\${HOST_NAME}.json"
  Delete "$INSTDIR\${HOST_NAME}.firefox.json"
  Delete "$INSTDIR\hydra.ico"
  Delete "$INSTDIR\LICENSE"
  Delete "$INSTDIR\uninstall.exe"
  RMDir /r "$INSTDIR\extensions"
  RMDir /r "$INSTDIR\assets"
  ; Not /r: leaves user data behind if anything else ever lands in $INSTDIR.
  RMDir "$INSTDIR"

  Delete "$SMPROGRAMS\Hydra\${APP_NAME}.lnk"
  Delete "$SMPROGRAMS\Hydra\Uninstall ${APP_NAME}.lnk"
  RMDir "$SMPROGRAMS\Hydra"
  Delete "$DESKTOP\${APP_NAME}.lnk"

  ; "Launch on startup" login item (written by the app, autostart.rs), plus
  ; the Startup-folder .cmd that pre-0.2.x versions wrote.
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "${APP_NAME}"
  Delete "$SMSTARTUP\hydra.cmd"

  DeleteRegKey HKCU "${UNINST_KEY}"
SectionEnd
