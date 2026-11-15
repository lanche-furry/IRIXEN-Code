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
; The installer no longer touches %APPDATA% itself. It used to copy the old
; config aside and delete it, which is textbook "hijacker" behaviour as far as
; generic antivirus heuristics go: Avira flagged the signed Beta 0.2 installer
; as TR/Hijacker.Gen with 27 of 28 engines clean, and the same family name had
; already hit Beta 0.1. So the work is split:
;
;   installer — on a fresh install only (no exe in $INSTDIR, not an update),
;               drop an empty first-run.marker next to its own binary. Writing
;               to its own install directory is what an installer is for.
;   app       — on startup, consume the marker and archive the profile itself:
;               config.json becomes config.json.preinstall, the live copy and
;               the model cache are removed, so the wizard runs. See
;               config::adopt_fresh_install.
;
; A reinstall over an existing copy writes no marker and keeps every setting.
;
; Uninstall with "delete app data" checked: remove the real profile directory
; and the WebView2 cache ($LOCALAPPDATA\com.irixen.code) so the next install
; really is a fresh one. That stays here — the user opts into it, and it removes
; this app's own data rather than another application's.

!macro NSIS_HOOK_PREINSTALL
  ${If} $UpdateMode <> 1
    IfFileExists "$INSTDIR\${MAINBINARYNAME}.exe" irixen_prev_install 0
      CreateDirectory "$INSTDIR"
      FileOpen $0 "$INSTDIR\first-run.marker" w
      FileClose $0
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
