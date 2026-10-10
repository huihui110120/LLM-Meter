!include LogicLib.nsh

Var LlmMeterUpgrade

!macro NSIS_HOOK_PREINSTALL
  StrCpy $LlmMeterUpgrade "0"
  ${If} ${FileExists} "$INSTDIR\llm-meter.exe"
    StrCpy $LlmMeterUpgrade "1"
    DetailPrint "Stopping the existing per-user LLM Meter daemon"
    ClearErrors
    ExecWait '"$INSTDIR\llm-meter.exe" _prepare-update' $0
    ${If} ${Errors}
      DetailPrint "The existing daemon shutdown helper could not be started"
      SetErrorLevel 1
      Abort
    ${ElseIf} $0 != 0
      DetailPrint "The existing daemon could not be stopped safely (exit code $0)"
      SetErrorLevel $0
      Abort
    ${EndIf}
  ${ElseIf} ${FileExists} "$INSTDIR\llm-meterd.exe"
    DetailPrint "An incomplete existing installation has no shutdown helper"
    SetErrorLevel 1
    Abort
  ${EndIf}
!macroend

!macro NSIS_HOOK_POSTINSTALL
  ClearErrors
  ${If} $LlmMeterUpgrade == "1"
    DetailPrint "Restoring the existing per-user LLM Meter daemon task"
    ExecWait '"$INSTDIR\llm-meter.exe" _post-update' $0
  ${Else}
    DetailPrint "Registering the per-user LLM Meter daemon task"
    ExecWait '"$INSTDIR\llm-meter.exe" autostart enable' $0
  ${EndIf}
  ${If} ${Errors}
    DetailPrint "The daemon task helper could not be started; it can be retried from the CLI"
  ${ElseIf} $0 != 0
    DetailPrint "The daemon task could not be reconciled (exit code $0); it can be retried from the CLI"
  ${EndIf}
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  DetailPrint "Stopping the per-user LLM Meter daemon"
  ClearErrors
  ExecWait '"$INSTDIR\llm-meter.exe" _prepare-update' $0
  ${If} ${Errors}
    DetailPrint "The daemon shutdown helper could not be started"
    SetErrorLevel 1
    Abort
  ${ElseIf} $0 != 0
    DetailPrint "The daemon could not be stopped safely (exit code $0)"
    SetErrorLevel $0
    Abort
  ${EndIf}

  DetailPrint "Unregistering the per-user LLM Meter daemon task"
  ClearErrors
  ExecWait '"$INSTDIR\llm-meter.exe" autostart disable' $0
  ${If} ${Errors}
    DetailPrint "The daemon task helper could not be started"
    SetErrorLevel 1
    Abort
  ${ElseIf} $0 != 0
    DetailPrint "The daemon task could not be removed cleanly (exit code $0)"
    SetErrorLevel $0
    Abort
  ${EndIf}
!macroend
