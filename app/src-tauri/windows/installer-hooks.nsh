; Sidescreen's Windows installer: after installing, let the Mac reach the app on private networks
; (its "please connect" request), and make sure Moonlight (the viewer that shows the Mac's
; display) is there too, using Windows' own package manager.

; The small library Windows loads into Moonlight so its stream window is out of sight from the
; moment it is made (crates/viewer-hook; built before the bundle by beforeBundleCommand in
; tauri.windows.conf.json). Defined here, where __FILEDIR__ is this file's folder.
!define SIDESCREEN_VIEWER_HOOK "${__FILEDIR__}\..\..\..\target\release\sidescreen_viewer_hook.dll"

!macro NSIS_HOOK_POSTINSTALL
  ; Beside the app, also on an update. Should Moonlight still have it loaded, the old one stays.
  ClearErrors
  SetOverwrite try
  File "/oname=$INSTDIR\sidescreen_viewer_hook.dll" "${SIDESCREEN_VIEWER_HOOK}"
  SetOverwrite on
  IfErrors 0 +2
    DetailPrint "sidescreen_viewer_hook.dll is in use (by a Moonlight still running); the one there stays."
  ; An in-place update (the app's updater runs this installer with /UPDATE) keeps the install
  ; folder, so the firewall rule still matches and nothing needs asking again.
  ${If} $UpdateMode <> 1
    ; The install is per-user, so the firewall rule needs one elevated step (a single UAC prompt:
    ; delete then add, so a reinstall stays tidy). If it is declined the install carries on and
    ; Windows asks by itself when the app first listens.
    ExecShell "runas" "cmd.exe" '/c netsh advfirewall firewall delete rule name="Sidescreen" & netsh advfirewall firewall add rule name="Sidescreen" dir=in action=allow program="$INSTDIR\sidescreen-app.exe" enable=yes profile=private' SW_HIDE
    IfFileExists "$PROGRAMFILES64\Moonlight Game Streaming\Moonlight.exe" moonlight_ready 0
    IfFileExists "$PROGRAMFILES\Moonlight Game Streaming\Moonlight.exe" moonlight_ready 0
      DetailPrint "Installing Moonlight, the viewer Sidescreen uses..."
      nsExec::ExecToLog 'winget install --id MoonlightGameStreamingProject.Moonlight --exact --silent --accept-source-agreements --accept-package-agreements'
      Pop $0
      StrCmp $0 "0" moonlight_ready 0
        DetailPrint "Moonlight could not be installed automatically. Sidescreen will offer to install it when it opens."
    moonlight_ready:
  ${EndIf}
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  ; Gone at the next restart should a Moonlight still have it.
  Delete /REBOOTOK "$INSTDIR\sidescreen_viewer_hook.dll"
  ExecShell "runas" "cmd.exe" '/c netsh advfirewall firewall delete rule name="Sidescreen"' SW_HIDE
!macroend
