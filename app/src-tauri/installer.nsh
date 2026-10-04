; Installs, updates and removes the ctxremote-service Windows service.

!macro NSIS_HOOK_PREINSTALL
  ; Stop a running service so its EXE can be overwritten (no-op on first install).
  IfFileExists "$INSTDIR\ctxremote-service.exe" 0 +3
    nsExec::ExecToLog '"$INSTDIR\ctxremote-service.exe" --stop'
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
  nsExec::ExecToLog '"$INSTDIR\ctxremote-service.exe" --install'
  Pop $0
  ; /SD: a silent update has nobody to click the box away.
  ${If} $0 != 0
    MessageBox MB_OK|MB_ICONEXCLAMATION "Der CTXRemote-Dienst konnte nicht eingerichtet werden (Fehlercode $0).$\r$\nDie Installation wurde abgeschlossen. Den Dienst können Sie später als Administrator mit $\"ctxremote-service.exe --install$\" einrichten." /SD IDOK
  ${EndIf}
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  nsExec::ExecToLog '"$INSTDIR\ctxremote-service.exe" --uninstall'
  Pop $0
!macroend
