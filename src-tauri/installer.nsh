; NSIS hooks for the Windows installer (wired up by
; bundle.windows.nsis.installerHooks in tauri.conf.json).
;
; Why this file exists: the app stores its profile in %APPDATA%\IRIXEN Code
; (config::base_dir) while the stock template only clears the bundle-identifier
; folder ("com.irixen.code"). A config.json left behind by an earlier build, a
; local test run or a previous install therefore survives every install —
; `onboarding_completed` is already true, and the first-run setup never appears
; on what looks like a fresh install.
;
; Fresh install into a directory that has no IRIXEN Code yet ($INSTDIR holds no
; exe when PREINSTALL runs) — the profile on disk can only be residue from an
; earlier version, a local test run or a leftover uninstall, and it would mark
; setup as done. Keep it as config.json.preinstall, then drop it so the wizard
; runs. A reinstall over an existing copy keeps every setting.
;
; Uninstall with "delete app data" checked: remove the real profile directory
; and the WebView2 cache ($LOCALAPPDATA\com.irixen.code) so the next install
; really is a fresh one.

!macro NSIS_HOOK_PREINSTALL
  ${If} $UpdateMode <> 1
    IfFileExists "$INSTDIR\${MAINBINARYNAME}.exe" irixen_prev_install 0
      SetShellVarContext current
      CopyFiles /SILENT "$APPDATA\IRIXEN Code\config.json" "$APPDATA\IRIXEN Code\config.json.preinstall"
      Delete "$APPDATA\IRIXEN Code\config.json"
      Delete "$APPDATA\IRIXEN Code\model-cache.json"
    irixen_prev_install:
  ${EndIf}
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  ${If} $DeleteAppDataCheckboxState = 1
  ${AndIf} $UpdateMode <> 1
    SetShellVarContext current
    RmDir /r "$APPDATA\IRIXEN Code"
    RmDir /r "$LOCALAPPDATA\com.irixen.code"
  ${EndIf}
!macroend
