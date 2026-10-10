# Windows Integration Baseline

> Version: v0.1  
> Status: implementation baseline  
> Supported architecture: `x86_64-pc-windows-msvc`

This document defines the Windows process, security, packaging, and support
contract for LLM Meter. The provider-neutral domain, SQLite schema, snapshot
DTOs, and JSON-RPC methods remain shared with Linux; only the operating-system
adapters differ.

## 1. Supported systems

The initial Windows release supports:

- Windows 10 22H2 and maintained Windows 11 releases;
- x64 processors, including Windows on ARM running the x64 package under
  emulation;
- an interactive, non-administrator user session;
- Microsoft Edge WebView2, installed by the NSIS bootstrapper when necessary.

Native ARM64, MSI enterprise deployment, Windows Server, and a system-wide
Windows Service are outside the first release. Windows 7 and 8 are unsupported.

## 2. Runtime topology

```text
llm-meter-desktop.exe (Tauri, tray, single instance)
                     │
                     │ JSON-RPC over a private named pipe
                     ▼
llm-meterd.exe (per-user background agent, single instance)
       ├── SQLite
       ├── Windows Credential Manager
       ├── provider scheduler
       ├── alerts and notifications
       ├── local Codex collector
       └── authenticated loopback relay proxy
```

The daemon is not embedded in the WebView process. Closing the popup or main
window does not stop synchronization, local proxying, or alert evaluation.
The desktop and CLI connect through the same versioned local IPC contract.

The Windows daemon runs as the signed-in user, never as `LocalSystem`. This is
required because credentials and desktop notifications belong to the user's
interactive session. A conventional Windows Service would run in Session 0
and would see the wrong credential and notification context.

## 3. Startup and single-instance behavior

The primary daemon startup mechanism is a per-user Task Scheduler task created
by `llm-meter autostart enable`. It uses an interactive user token at limited
privilege, a logon trigger, restart-on-failure, and an `IgnoreNew` instance
policy. Registration is idempotent and immediately starts the task. The
corresponding `disable` operation disables and stops it, while `status` reports
the structured task state, executable path, and last result.

The desktop also provides a recovery path when that task has not run or the
daemon has exited:

1. probe the daemon with `system/version`;
2. if it is offline, start the sibling `llm-meterd.exe` from the installation
   directory;
3. wait for the named pipe for at most five seconds;
4. verify the IPC version before sending the requested method.

This bounded recovery complements rather than replaces the scheduled task. The
daemon owns the cross-process singleton guard, so concurrent recovery attempts
cannot create multiple agents. The desktop additionally uses Tauri's
single-instance plugin: launching it again activates the existing process
instead of creating another widget or tray icon.

The NSIS installer invokes the CLI idempotently to register the daemon task.
Before an upgrade or uninstall replaces/removes binaries, the CLI requests
`system/shutdown`, waits on the SID-verified daemon process handle, and only
uses Task Scheduler `Stop` after the graceful-exit deadline. Uninstall aborts
if process exit cannot be confirmed, then removes the task only after the
daemon has stopped. Upgrade reconciliation preserves a missing or manually
disabled task rather than silently re-enabling it.

The installer does not silently register desktop login startup. If the user or
a future installer option starts the desktop at login, it must pass `--hide`,
which creates the tray without opening a window.

The normal Windows surface mirrors the two Noctalia plugin layers instead of
using the tray as the primary interface:

- `BarWidget.qml` maps to a 320 x 48 always-on-top compact widget. It displays
  the configured account, lowest remaining quota, optional local usage, and
  compact trend. The widget can be dragged and persists its last valid screen
  position.
- A left click toggles the `Panel.qml`-equivalent 420 x 480 panel next to the
  widget. Closing the panel leaves the compact widget and daemon running.
- A right click opens the main window. The tray remains a recovery path for
  showing the widget, refreshing data, opening the main window, and quitting
  the desktop client.

The tray provides these baseline actions:

- **Show compact widget**: restore the Windows `BarWidget` surface;
- **Open LLM Meter**: open the main window;
- **Refresh now**: request a daemon refresh without blocking the UI;
- **Quit desktop**: close only the desktop client; the daemon remains available
  to the CLI and continues background work.

## 4. Local IPC security

Linux uses a private Unix domain socket. Windows uses a named pipe scoped to
the current user. The server must apply an explicit DACL that permits the
current user SID and necessary system principals only; including a SID in the
pipe name is not by itself an access control mechanism.

The transport keeps the existing newline-delimited JSON-RPC framing and the
`system/version` handshake. No fixed localhost TCP port is used for control
IPC. A second local user must not be able to connect or read responses. Because
the SID-derived pipe name is predictable, another local user can pre-create an
object with the same name and deny daemon startup. First-instance enforcement
and mutual process-SID checks turn that namespace race into availability-only
denial of service: the client refuses the impostor, so no secret is disclosed.

Secrets may pass transiently from a credential input control to the daemon over
this protected pipe. They must never be persisted in WebView state, SQLite,
configuration, logs, telemetry, diagnostics, or snapshot DTOs.

## 5. Files and credentials

The default Windows data root is:

```text
%LOCALAPPDATA%\LLM Meter\
├── config.toml
├── data\
│   ├── llm-meter.sqlite3
│   ├── llm-meter.sqlite3-wal
│   └── llm-meter.sqlite3-shm
└── logs\
    └── llm-meterd.log
```

`LLM_METER_HOME` can override this root for development and isolated tests.
The named pipe is a kernel object and has no socket file under this directory.

API keys, relay tokens, and OAuth credentials are stored as Generic
Credentials in Windows Credential Manager. SQLite stores only opaque
credential references. Credential Manager's generic credential blob has a
finite size (approximately 2560 bytes), so representative maximum OAuth token
fixtures must be tested. An oversized credential is rejected with the stable
`credential_too_large` error code; it never falls back to a plaintext file.

Removing a connection removes every credential entry owned by that connection.
The baseline uninstaller stops and removes the scheduled task and program files
but intentionally retains `%LOCALAPPDATA%\LLM Meter` and Credential Manager
entries. Users who want full credential cleanup should remove their connections
before uninstalling; a future explicit “remove user data” option must remain
separate from the default uninstall path.

## 6. Codex integration

Codex-owned session logs are discovered under:

```text
%USERPROFILE%\.codex\sessions\YYYY\MM\DD\*.jsonl
```

Historical JSONL parsing and local-day accounting use the same implementation
and data semantics as Linux. Linux's `/proc/<pid>/fd` discovery does not exist
on Windows; active-session reporting must use the Windows collector rather
than silently treating an unavailable collector as an authoritative zero.

The Codex executable may be installed as `codex.exe` or as an npm-generated
`codex.cmd`. Executable discovery must support both without constructing an
unquoted shell command.

## 7. Browser and notifications

Only validated `https` authentication URLs may be opened. Windows delegates
them to the user's default browser through the operating-system opener; Linux
uses `xdg-open`, and macOS uses `open`.

Windows toast notifications run in the interactive user session. The installed
application must use a stable AppUserModelID so notifications are grouped under
LLM Meter. Toast delivery and tray behavior require a real interactive Windows
VM for acceptance testing; a headless CI runner can verify compilation but not
visible presentation.

## 8. Distribution formats

### NSIS installer

The NSIS installer is the primary Windows distribution. It is a current-user
install and does not require elevation. It includes the desktop application,
CLI, and daemon sidecars; creates the application shortcuts; registers the
per-user daemon task; and downloads the WebView2 bootstrapper silently only
when WebView2 is unavailable.

The Windows icon is mechanically derived from the repository's source PNG by
the checked-in icon conversion script during the release build. No generated
artwork is used.

Production releases should Authenticode-sign the installer and every installed
executable. Checksums complement signing but do not replace it.

The current workflow has no signing credential or signing service. It therefore
publishes Windows files with `unsigned-preview` in their names. These are
preview artifacts, not production-signed releases, even when their SHA-256
checksums match.

### Portable ZIP

The portable ZIP contains:

```text
bin\llm-meter.exe
bin\llm-meterd.exe
bin\llm-meter-desktop.exe
README.md
windows-integration-v0.1.md
LICENSE
config.example.toml
```

Portable means “no installer”; it does not mean that data and credentials live
beside the executable. The ZIP still uses `%LOCALAPPDATA%` and Windows
Credential Manager. It does not provision WebView2, shortcuts, login startup,
or an uninstall entry. To enable the daemon's logon task from an extracted ZIP,
run `bin\llm-meter.exe autostart enable`; run the corresponding `disable`
command before deleting or moving the extracted directory.

MSI is deferred until enterprise deployment, Group Policy, repair, and rollback
requirements justify a separate WiX profile.

## 9. Building on Windows

Install the stable Rust MSVC toolchain and the prerequisites required by Tauri
v2. From the repository root:

```powershell
cargo check --workspace --all-targets --locked
cargo test --workspace --locked
cargo build --workspace --release --locked

$target = (rustc -vV | Select-String '^host: ').Line.Substring(6)
New-Item -ItemType Directory -Force apps/desktop/src-tauri/binaries | Out-Null
Copy-Item target/release/llm-meter.exe `
  "apps/desktop/src-tauri/binaries/llm-meter-$target.exe"
Copy-Item target/release/llm-meterd.exe `
  "apps/desktop/src-tauri/binaries/llm-meterd-$target.exe"

powershell.exe -NoProfile -ExecutionPolicy Bypass -File `
  apps/desktop/src-tauri/scripts/generate-windows-icon.ps1
cargo install tauri-cli --version '=2.11.4' --locked
Push-Location apps/desktop
cargo tauri icon src-tauri/icons/icon.png `
  --output src-tauri/icons/generated
cargo tauri build --bundles nsis -- --locked
Pop-Location
```

The release workflow performs the same native Windows build and also creates a
portable ZIP and combined `SHA256SUMS` file.

## 10. Diagnostics

Windows diagnostics should report, without exposing secrets:

- daemon process and singleton state;
- named-pipe reachability and IPC version;
- application and daemon executable versions;
- `%LOCALAPPDATA%` data root and database health;
- Credential Manager write/read/delete probe result;
- WebView2 runtime version;
- the per-user scheduled task state, registered path, and last result;
- Codex executable and session directory discovery;
- notification identity availability;
- whether the relay proxy is bound to loopback only.

Common recovery actions are:

1. start the desktop or CLI again to trigger bounded daemon startup;
2. verify that `llm-meterd.exe` exists beside the desktop executable;
3. check `%LOCALAPPDATA%\LLM Meter\logs\llm-meterd.log`;
4. repair/install WebView2 if the desktop cannot create a window;
5. verify that endpoint-security software has not quarantined an executable;
6. for preview builds, account for the expected unknown-publisher warning.

## 11. Release acceptance

A Windows release is complete only when all of the following pass on a clean
standard-user installation:

- Win10 22H2 and Win11 x64 install, launch, upgrade, and uninstall tests;
- one daemon and one tray process after repeated launches;
- hidden login launch without a surprise popup;
- daemon recovery within the bounded wait;
- another local user denied access to the named pipe;
- Credential Manager put/get/delete, reboot persistence, and connection-delete
  cleanup with maximum representative token fixtures;
- no secret material in SQLite, logs, diagnostics, or snapshot responses;
- popup, main window, tray menu, browser authentication, and visible toast tests;
- provider refresh, stale/offline behavior, budgets, and loopback proxy tests;
- paths containing spaces, Chinese characters, and other Unicode characters;
- valid Authenticode signatures and published SHA-256 checksums.
