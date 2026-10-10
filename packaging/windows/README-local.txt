LLM Meter local Windows build
=============================

Double-click "LLM Meter.exe".
The desktop process starts the sibling llm-meterd.exe automatically.

The 320 x 48 compact widget is the Windows equivalent of Noctalia BarWidget:
  Click       Toggle the full status panel
  Drag        Move the widget (its position is remembered)
  Right-click Open the main window
The tray icon is a fallback for restoring the widget, refreshing, and quitting.

Application data and logs are stored under:
  %LOCALAPPDATA%\LLM Meter

Credentials are stored in Windows Credential Manager. They are not stored
beside these executables.

Quitting the tray application closes only the desktop. The background daemon
continues running by design. To stop a running daemon from PowerShell, run:
  .\llm-meter.exe _prepare-update

This directory is an unsigned local release build, not a production installer.
