; PowerCost installer hooks (NSIS). Per-user install, no admin rights.

!macro NSIS_HOOK_PREINSTALL
!macroend

!macro NSIS_HOOK_POSTINSTALL
!macroend

!macro NSIS_HOOK_PREUNINSTALL
!macroend

; The "Start with Windows" option in the app writes a per-user Run value. Remove it when the
; program is uninstalled so Windows is not left trying to start a program that no longer exists.
!macro NSIS_HOOK_POSTUNINSTALL
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "PowerCost"
!macroend