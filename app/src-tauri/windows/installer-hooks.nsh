; Sidescreen's Windows installer: after installing, make sure Moonlight (the viewer that shows
; the Mac's display) is there too, using Windows' own package manager.

!macro NSIS_HOOK_POSTINSTALL
  IfFileExists "$PROGRAMFILES64\Moonlight Game Streaming\Moonlight.exe" moonlight_ready 0
  IfFileExists "$PROGRAMFILES\Moonlight Game Streaming\Moonlight.exe" moonlight_ready 0
    DetailPrint "Installing Moonlight, the viewer Sidescreen uses..."
    nsExec::ExecToLog 'winget install --id MoonlightGameStreamingProject.Moonlight --exact --silent --accept-source-agreements --accept-package-agreements'
    Pop $0
    StrCmp $0 "0" moonlight_ready 0
      DetailPrint "Moonlight could not be installed automatically. Sidescreen will offer to install it when it opens."
  moonlight_ready:
!macroend
