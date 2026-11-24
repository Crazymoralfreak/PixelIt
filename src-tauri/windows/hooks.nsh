; This file is included before the installer pages are declared.

; The finish page offers a desktop shortcut; the box starts unticked.
; (A silent or passive install has no finish page and creates the shortcut
; unless it is run with /NS.)
!define MUI_FINISHPAGE_SHOWREADME_NOTCHECKED

; The "Open in PixelIt" item of the Explorer context menu is switched on in
; the settings of the application and written for the current user only.
; A real uninstall removes it; an update keeps what the user chose.
!macro NSIS_HOOK_PREUNINSTALL
  ${If} $UpdateMode <> 1
    DeleteRegKey HKCU "Software\Classes\SystemFileAssociations\image\shell\PixelIt"
    DeleteRegKey HKCU "Software\Classes\Directory\shell\PixelIt"
  ${EndIf}
!macroend

; PixelIt keeps recipes, settings and downloaded models in %AppData%\PixelIt,
; not in the identifier-based folder that the stock uninstaller knows about.
; Remove it only when the user ticked "Delete the application data" and this
; is a real uninstall, not an update.
!macro NSIS_HOOK_POSTUNINSTALL
  ${If} $DeleteAppDataCheckboxState = 1
  ${AndIf} $UpdateMode <> 1
    SetShellVarContext current
    RMDir /r "$APPDATA\PixelIt"
  ${EndIf}
!macroend
