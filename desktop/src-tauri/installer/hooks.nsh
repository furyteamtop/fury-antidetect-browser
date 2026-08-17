; SPDX-License-Identifier: AGPL-3.0-or-later
; Copyright 2026 Bogdan Shapovalov and the Fury authors
;
; Close what is running before writing over it.
;
; Windows will not let anybody overwrite a running executable, and an installer
; that tries gets this, mid-progress-bar, with Abort / Retry / Ignore:
;
;   Error opening file for writing:
;   C:\Users\<user>\AppData\Local\Fury\fury-agent.exe
;
; Met on 16.08.2026 installing over a Fury that was open. Tauri's own template
; already closes the main application -- fury-desktop.exe -- and it is the
; SIDECAR that stops it: fury-agent.exe is spawned by the shell, outlives the
; window by design, and nothing in the generated script knows it exists.
;
; None of the three buttons is a good answer for the person seeing it. Abort
; leaves a half-installed application. Ignore leaves the OLD agent beside the
; new shell, which is the worst outcome and the one that looks like it worked.
; Retry works only if they know to go and kill a process they never started.
;
; taskkill rather than a plugin, because it ships with Windows and this needs no
; download. /F because the agent holds a named pipe and will not close politely
; on a signal it never receives; nothing it holds needs flushing -- the vault is
; a database that commits per write, and the profiles it manages are separate
; processes that survive it.
;
; Failure is deliberately not checked. taskkill returns non-zero when there is
; no such process, which is the ordinary case: most installs are onto a machine
; where Fury is not running. An error there would turn the common path into a
; scary dialog, which is exactly what this file exists to remove.

!macro NSIS_HOOK_PREINSTALL
  DetailPrint "Closing Fury if it is running..."
  nsExec::Exec 'taskkill /F /IM fury-agent.exe'
  Pop $0
  nsExec::Exec 'taskkill /F /IM fury-desktop.exe'
  Pop $0
  ; The kill returns as soon as the request is made, not when the handles are
  ; released. Writing immediately still loses the race often enough to matter.
  Sleep 1200
!macroend

!macro NSIS_HOOK_POSTINSTALL
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  ; Same reason, and more pressing: an uninstaller that cannot delete the agent
  ; leaves the directory behind and reports success.
  DetailPrint "Closing Fury if it is running..."
  nsExec::Exec 'taskkill /F /IM fury-agent.exe'
  Pop $0
  nsExec::Exec 'taskkill /F /IM fury-desktop.exe'
  Pop $0
  Sleep 1200
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
!macroend
