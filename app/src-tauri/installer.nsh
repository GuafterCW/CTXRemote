; Installs, updates and removes the remocto-service Windows service, and
; replaces an installation from before the rename (CTXRemote).

; CTXRemote, as Remocto was called before: its service goes and its own
; uninstaller removes the rest, so two services never share one device ID.
; Its data folder stays; the new service takes it over (same device ID).
!macro REMOCTO_REMOVE_FORMER
  Push $R8
  Push $R9
  ReadRegStr $R8 HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\CTXRemote" "UninstallString"
  ReadRegStr $R9 HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\CTXRemote" "InstallLocation"
  ${If} $R9 == ""
    StrCpy $R9 "$PROGRAMFILES64\CTXRemote"
  ${EndIf}
  ${If} ${FileExists} "$R9\ctxremote-service.exe"
    nsExec::ExecToLog '"$R9\ctxremote-service.exe" --uninstall'
    Pop $0
  ${EndIf}
  ${If} $R8 != ""
    nsExec::Exec 'taskkill /F /IM "CTXRemote.exe"'
    Pop $0
    ; _?= keeps the uninstaller in place, so this waits until it is done.
    ExecWait '$R8 /S _?=$R9' $0
    Delete "$R9\uninstall.exe"
    RMDir "$R9"
  ${EndIf}
  Pop $R9
  Pop $R8
!macroend

!macro NSIS_HOOK_PREINSTALL
  !insertmacro REMOCTO_REMOVE_FORMER
  ; Stop a running service so its EXE can be overwritten (no-op on first install).
  IfFileExists "$INSTDIR\remocto-service.exe" 0 +3
    nsExec::ExecToLog '"$INSTDIR\remocto-service.exe" --stop'
    Pop $0
  ; A silent update comes from the service, which runs as SYSTEM in session 0.
  ; From there the template's app check (Restart Manager) cannot close the app
  ; in the user's session and aborts the update, so close it here, in every
  ; session. The service starts it again after the update.
  ${If} ${Silent}
    nsExec::Exec 'taskkill /F /IM "${MAINBINARYNAME}.exe"'
    Pop $0
    Sleep 500
  ${EndIf}
!macroend

!macro NSIS_HOOK_POSTINSTALL
  nsExec::ExecToLog '"$INSTDIR\remocto-service.exe" --install'
  Pop $0
  ; /SD: a silent update has nobody to click the box away.
  ${If} $0 != 0
    MessageBox MB_OK|MB_ICONEXCLAMATION "Der Remocto-Dienst konnte nicht eingerichtet werden (Fehlercode $0).$\r$\nDie Installation wurde abgeschlossen. Den Dienst können Sie später als Administrator mit $\"remocto-service.exe --install$\" einrichten." /SD IDOK
  ${EndIf}
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  nsExec::ExecToLog '"$INSTDIR\remocto-service.exe" --uninstall'
  Pop $0
!macroend
