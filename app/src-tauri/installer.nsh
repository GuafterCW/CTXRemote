; Installs, updates and removes the ctxremote-service Windows service.

!macro NSIS_HOOK_PREINSTALL
  ; Stop a running service so its EXE can be overwritten (no-op on first install).
  IfFileExists "$INSTDIR\ctxremote-service.exe" 0 +3
    nsExec::ExecToLog '"$INSTDIR\ctxremote-service.exe" --stop'
    Pop $0
!macroend

!macro NSIS_HOOK_POSTINSTALL
  nsExec::ExecToLog '"$INSTDIR\ctxremote-service.exe" --install'
  Pop $0
  ${If} $0 != 0
    MessageBox MB_OK|MB_ICONEXCLAMATION "Der CTXRemote-Dienst konnte nicht eingerichtet werden (Fehlercode $0).$\r$\nDie Installation wurde abgeschlossen. Den Dienst können Sie später als Administrator mit $\"ctxremote-service.exe --install$\" einrichten."
  ${EndIf}
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  nsExec::ExecToLog '"$INSTDIR\ctxremote-service.exe" --uninstall'
  Pop $0
!macroend
