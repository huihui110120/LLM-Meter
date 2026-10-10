use serde::Serialize;
use std::{
    ffi::c_void,
    io::{self, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use windows::{
    Win32::{
        Foundation::{CloseHandle, HANDLE, VARIANT_FALSE, VARIANT_TRUE},
        Security::{
            GetLengthSid, GetTokenInformation, IsValidSid, TOKEN_QUERY, TOKEN_USER, TokenUser,
        },
        System::{
            Com::{
                CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
                CoUninitialize,
            },
            TaskScheduler::{
                IExecAction, ILogonTrigger, IRegisteredTask, ITaskFolder, ITaskService,
                TASK_ACTION_EXEC, TASK_CREATE_OR_UPDATE, TASK_INSTANCES_IGNORE_NEW,
                TASK_LOGON_INTERACTIVE_TOKEN, TASK_RUNLEVEL_LUA, TASK_STATE_DISABLED,
                TASK_STATE_QUEUED, TASK_STATE_READY, TASK_STATE_RUNNING, TASK_STATE_UNKNOWN,
                TASK_TRIGGER_LOGON, TaskScheduler,
            },
            Threading::{GetCurrentProcess, OpenProcessToken},
            Variant::VARIANT,
        },
    },
    core::{BSTR, Error as WindowsError, Interface},
};

const TASK_PREFIX: &str = "LLM Meter Daemon";
const TASK_DESCRIPTION: &str = "Starts the per-user LLM Meter daemon at sign-in.";

#[derive(Clone, Copy, Debug, clap::ValueEnum)]
pub enum DesktopEnvironment {
    Windows,
}

#[derive(Clone, Copy, Debug, clap::ValueEnum)]
pub enum TopBar {
    Tray,
}

pub struct Options {
    pub desktop: Option<DesktopEnvironment>,
    pub bar: Option<TopBar>,
    pub non_interactive: bool,
    pub dry_run: bool,
}

#[derive(Debug, Serialize)]
pub struct AutostartStatus {
    pub backend: &'static str,
    pub registration: String,
    pub installed: bool,
    pub enabled: bool,
    pub active: bool,
    pub state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub executable: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub executable_matches_installation: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_result: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_result_hex: Option<String>,
    pub run_level: &'static str,
    pub logon_type: &'static str,
}

pub fn run(options: Options) -> Result<(), Box<dyn std::error::Error>> {
    if !matches!(options.desktop, None | Some(DesktopEnvironment::Windows)) {
        return Err("Windows setup only supports the Windows desktop profile".into());
    }
    if !matches!(options.bar, None | Some(TopBar::Tray)) {
        return Err("Windows setup only supports the tray profile".into());
    }

    let daemon = daemon_path()?;
    let task_name = task_name()?;
    println!("LLM Meter Windows integration setup");
    println!("\nInstallation plan:");
    println!("  Install scope: current user (no elevation)");
    println!("  Daemon:        Task Scheduler interactive-token task");
    println!("  Task:          {task_name}");
    println!("  Executable:    {}", daemon.display());

    if options.dry_run {
        println!("\nDry run complete; no system state was changed.");
        return Ok(());
    }
    if !options.non_interactive && !confirm("Enable daemon startup for this user?", true)? {
        println!("Setup cancelled.");
        return Ok(());
    }

    let status = set_autostart(true)?;
    println!(
        "\nLLM Meter daemon startup is {} (state: {}).",
        if status.enabled {
            "enabled"
        } else {
            "disabled"
        },
        status.state
    );
    Ok(())
}

pub fn autostart_status() -> Result<AutostartStatus, Box<dyn std::error::Error>> {
    with_task_service(|_service, folder, identity| {
        let name = task_name_for_scope(&identity.scope);
        let task = match unsafe { folder.GetTask(&BSTR::from(name.as_str())) } {
            Ok(task) => task,
            Err(error) if task_missing(&error) => return Ok(missing_status(name)),
            Err(error) => return Err(error.into()),
        };
        status_from_task(name, &task)
    })
}

pub fn set_autostart(enabled: bool) -> Result<AutostartStatus, Box<dyn std::error::Error>> {
    with_task_service(|service, folder, identity| {
        let name = task_name_for_scope(&identity.scope);
        if enabled {
            let daemon = daemon_path()?;
            let task = register_task(service, folder, &name, &identity.sid, &daemon, true)?;
            unsafe {
                task.Run(&VARIANT::default())?;
            }
            status_from_task(name, &task)
        } else {
            let task = match unsafe { folder.GetTask(&BSTR::from(name.as_str())) } {
                Ok(task) => task,
                Err(error) if task_missing(&error) => return Ok(missing_status(name)),
                Err(error) => return Err(error.into()),
            };
            unsafe {
                let state = task.State()?;
                task.SetEnabled(VARIANT_FALSE)?;
                if state == TASK_STATE_RUNNING || state == TASK_STATE_QUEUED {
                    task.Stop(0)?;
                }
                folder.DeleteTask(&BSTR::from(name.as_str()), 0)?;
            }
            Ok(missing_status(name))
        }
    })
}

pub async fn prepare_update() -> Result<(), Box<dyn std::error::Error>> {
    let endpoint = llm_meter_ipc::endpoint()?;
    let mut client = match llm_meter_ipc::Client::connect(&endpoint).await {
        Ok(client) => client,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let task_was_running = stop_registered_task()?;
            tokio::time::sleep(Duration::from_millis(250)).await;
            match llm_meter_ipc::Client::connect(&endpoint).await {
                Ok(client) => client,
                Err(retry_error) if retry_error.kind() == io::ErrorKind::NotFound => {
                    if task_was_running {
                        return Err(
                            "the daemon task was running, but no verified process handle was available; retry after it stops"
                                .into(),
                        );
                    }
                    println!("No daemon process is running; its binaries are ready to update.");
                    return Ok(());
                }
                Err(retry_error) => {
                    return Err(format!(
                        "could not verify the daemon after stopping its task: {retry_error}"
                    )
                    .into());
                }
            }
        }
        Err(error) => {
            return Err(format!("could not connect to the daemon safely: {error}").into());
        }
    };

    let pid = client.server_pid();
    let shutdown = tokio::time::timeout(
        Duration::from_secs(3),
        client.call("system/shutdown", serde_json::json!({})),
    )
    .await;
    match shutdown {
        Ok(Ok(_)) => println!("Requested a graceful shutdown from daemon process {pid}."),
        Ok(Err(error)) => eprintln!(
            "The daemon did not acknowledge graceful shutdown ({error}); requesting a task stop if it remains active."
        ),
        Err(_) => eprintln!(
            "The daemon shutdown request timed out; requesting a task stop if it remains active."
        ),
    }

    if wait_for_server_exit(&client, Duration::from_secs(3)).await? {
        println!("The daemon exited cleanly and its binaries are ready to update.");
        return Ok(());
    }

    stop_registered_task()?;
    if wait_for_server_exit(&client, Duration::from_secs(5)).await? {
        println!("The daemon task stopped and its binaries are ready to update.");
        return Ok(());
    }
    Err(format!("daemon process {pid} did not exit before the update deadline").into())
}

async fn wait_for_server_exit(
    client: &llm_meter_ipc::Client,
    timeout: Duration,
) -> io::Result<bool> {
    let deadline = Instant::now() + timeout;
    loop {
        if client.server_process_exited()? {
            return Ok(true);
        }
        if Instant::now() >= deadline {
            return Ok(false);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn stop_registered_task() -> Result<bool, Box<dyn std::error::Error>> {
    with_task_service(|_service, folder, identity| {
        let name = task_name_for_scope(&identity.scope);
        let task = match unsafe { folder.GetTask(&BSTR::from(name.as_str())) } {
            Ok(task) => task,
            Err(error) if task_missing(&error) => return Ok(false),
            Err(error) => return Err(error.into()),
        };
        let state = unsafe { task.State()? };
        let was_active = state == TASK_STATE_RUNNING || state == TASK_STATE_QUEUED;
        if was_active {
            unsafe {
                task.Stop(0)?;
            }
        }
        Ok(was_active)
    })
}

/// Reconcile the task definition after an installer replaces the binaries.
/// Missing registrations are not created implicitly, and a disabled task
/// remains disabled.
pub fn post_update() -> Result<(), Box<dyn std::error::Error>> {
    let before = autostart_status()?;
    if !before.installed {
        println!("No daemon task is registered; skipped Windows autostart reconciliation.");
        return Ok(());
    }
    with_task_service(|service, folder, identity| {
        let name = task_name_for_scope(&identity.scope);
        let daemon = daemon_path()?;
        let task = register_task(
            service,
            folder,
            &name,
            &identity.sid,
            &daemon,
            before.enabled,
        )?;
        if before.enabled {
            unsafe {
                task.Run(&VARIANT::default())?;
            }
        }
        Ok(())
    })?;
    println!("Reconciled the Windows daemon task with the installed executable.");
    Ok(())
}

fn register_task(
    service: &ITaskService,
    folder: &ITaskFolder,
    name: &str,
    user_sid: &str,
    daemon: &Path,
    enabled: bool,
) -> Result<IRegisteredTask, Box<dyn std::error::Error>> {
    let definition = unsafe { service.NewTask(0)? };
    let registration = unsafe { definition.RegistrationInfo()? };
    unsafe {
        registration.SetAuthor(&BSTR::from("LLM Meter"))?;
        registration.SetDescription(&BSTR::from(TASK_DESCRIPTION))?;
    }

    let principal = unsafe { definition.Principal()? };
    unsafe {
        principal.SetUserId(&BSTR::from(user_sid))?;
        principal.SetLogonType(TASK_LOGON_INTERACTIVE_TOKEN)?;
        principal.SetRunLevel(TASK_RUNLEVEL_LUA)?;
    }

    let settings = unsafe { definition.Settings()? };
    unsafe {
        settings.SetEnabled(if enabled { VARIANT_TRUE } else { VARIANT_FALSE })?;
        settings.SetStartWhenAvailable(VARIANT_TRUE)?;
        settings.SetDisallowStartIfOnBatteries(VARIANT_FALSE)?;
        settings.SetStopIfGoingOnBatteries(VARIANT_FALSE)?;
        settings.SetExecutionTimeLimit(&BSTR::from("PT0S"))?;
        settings.SetMultipleInstances(TASK_INSTANCES_IGNORE_NEW)?;
        settings.SetRestartInterval(&BSTR::from("PT1M"))?;
        settings.SetRestartCount(5)?;
    }

    let triggers = unsafe { definition.Triggers()? };
    let trigger: ILogonTrigger = unsafe { triggers.Create(TASK_TRIGGER_LOGON)? }.cast()?;
    unsafe {
        trigger.SetId(&BSTR::from("CurrentUserLogon"))?;
        trigger.SetUserId(&BSTR::from(user_sid))?;
        trigger.SetDelay(&BSTR::from("PT10S"))?;
    }

    let actions = unsafe { definition.Actions()? };
    let action: IExecAction = unsafe { actions.Create(TASK_ACTION_EXEC)? }.cast()?;
    let executable = daemon.to_string_lossy();
    let working_directory = daemon
        .parent()
        .ok_or("daemon path has no parent directory")?;
    unsafe {
        action.SetPath(&BSTR::from(executable.as_ref()))?;
        action.SetArguments(&BSTR::from("--background"))?;
        action.SetWorkingDirectory(&BSTR::from(working_directory.to_string_lossy().as_ref()))?;
    }

    let task = unsafe {
        folder.RegisterTaskDefinition(
            &BSTR::from(name),
            &definition,
            TASK_CREATE_OR_UPDATE.0,
            &VARIANT::default(),
            &VARIANT::default(),
            TASK_LOGON_INTERACTIVE_TOKEN,
            &VARIANT::default(),
        )?
    };
    Ok(task)
}

fn status_from_task(
    name: String,
    task: &IRegisteredTask,
) -> Result<AutostartStatus, Box<dyn std::error::Error>> {
    let enabled = unsafe { task.Enabled()? } != VARIANT_FALSE;
    let state = unsafe { task.State()? };
    let last_result = unsafe { task.LastTaskResult()? };
    let definition = unsafe { task.Definition()? };
    let principal = unsafe { definition.Principal()? };
    let mut actual_run_level = TASK_RUNLEVEL_LUA;
    unsafe {
        principal.RunLevel(&mut actual_run_level)?;
    }
    let run_level = if actual_run_level == TASK_RUNLEVEL_LUA {
        "limited"
    } else {
        "highest"
    };
    let mut actual_logon_type = TASK_LOGON_INTERACTIVE_TOKEN;
    unsafe {
        principal.LogonType(&mut actual_logon_type)?;
    }
    let logon_type = if actual_logon_type == TASK_LOGON_INTERACTIVE_TOKEN {
        "interactive-token"
    } else {
        "unexpected"
    };
    let executable = task_executable(&definition)?;
    let expected = daemon_path().ok();
    let executable_matches_installation = match (executable.as_deref(), expected.as_deref()) {
        (Some(actual), Some(expected)) => Some(windows_paths_equal(actual, expected)),
        _ => None,
    };

    Ok(AutostartStatus {
        backend: "task-scheduler",
        registration: name,
        installed: true,
        enabled,
        active: state == TASK_STATE_RUNNING,
        state: task_state_name(state),
        executable,
        executable_matches_installation,
        last_result: Some(last_result),
        last_result_hex: Some(format!("0x{:08X}", last_result as u32)),
        run_level,
        logon_type,
    })
}

fn task_executable(
    definition: &windows::Win32::System::TaskScheduler::ITaskDefinition,
) -> Result<Option<String>, Box<dyn std::error::Error>> {
    let actions = unsafe { definition.Actions()? };
    let mut count = 0;
    unsafe {
        actions.Count(&mut count)?;
    }
    if count == 0 {
        return Ok(None);
    }
    let action = unsafe { actions.get_Item(1)? };
    let Ok(executable) = action.cast::<IExecAction>() else {
        return Ok(None);
    };
    let mut path = BSTR::new();
    unsafe {
        executable.Path(&mut path)?;
    }
    Ok(Some(path.to_string()))
}

fn missing_status(name: String) -> AutostartStatus {
    AutostartStatus {
        backend: "task-scheduler",
        registration: name,
        installed: false,
        enabled: false,
        active: false,
        state: "missing",
        executable: None,
        executable_matches_installation: None,
        last_result: None,
        last_result_hex: None,
        run_level: "limited",
        logon_type: "interactive-token",
    }
}

fn task_state_name(state: windows::Win32::System::TaskScheduler::TASK_STATE) -> &'static str {
    if state == TASK_STATE_UNKNOWN {
        "unknown"
    } else if state == TASK_STATE_DISABLED {
        "disabled"
    } else if state == TASK_STATE_QUEUED {
        "queued"
    } else if state == TASK_STATE_READY {
        "ready"
    } else if state == TASK_STATE_RUNNING {
        "running"
    } else {
        "unknown"
    }
}

fn task_missing(error: &WindowsError) -> bool {
    task_missing_code(error.code().0 as u32)
}

fn task_missing_code(code: u32) -> bool {
    matches!(code, 0x8007_0002 | 0x8007_0003)
}

struct UserIdentity {
    sid: String,
    scope: String,
}

fn current_user_identity() -> Result<UserIdentity, Box<dyn std::error::Error>> {
    let mut token = HANDLE::default();
    unsafe {
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token)?;
    }
    let token = OwnedHandle(token);
    let mut required = 0;
    let _ = unsafe { GetTokenInformation(token.0, TokenUser, None, 0, &mut required) };
    if required == 0 {
        return Err(WindowsError::from_win32().into());
    }
    let words = (required as usize).div_ceil(std::mem::size_of::<usize>());
    let mut buffer = vec![0usize; words];
    unsafe {
        GetTokenInformation(
            token.0,
            TokenUser,
            Some(buffer.as_mut_ptr().cast::<c_void>()),
            required,
            &mut required,
        )?;
    }
    let token_user = unsafe { &*buffer.as_ptr().cast::<TOKEN_USER>() };
    let sid = token_user.User.Sid;
    if !unsafe { IsValidSid(sid) }.as_bool() {
        return Err("the current process token contains an invalid user SID".into());
    }
    let length = unsafe { GetLengthSid(sid) } as usize;
    let bytes = unsafe { std::slice::from_raw_parts(sid.0.cast::<u8>(), length) };
    Ok(UserIdentity {
        sid: sid_string(bytes)?,
        scope: llm_meter_ipc::user_scope_id()?,
    })
}

struct OwnedHandle(HANDLE);

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        let _ = unsafe { CloseHandle(self.0) };
    }
}

fn sid_string(bytes: &[u8]) -> Result<String, Box<dyn std::error::Error>> {
    if bytes.len() < 8 {
        return Err("user SID is truncated".into());
    }
    let sub_authorities = bytes[1] as usize;
    let expected = 8usize
        .checked_add(
            sub_authorities
                .checked_mul(4)
                .ok_or("user SID is too large")?,
        )
        .ok_or("user SID is too large")?;
    if bytes.len() < expected {
        return Err("user SID sub-authorities are truncated".into());
    }
    let authority = bytes[2..8]
        .iter()
        .fold(0u64, |value, byte| (value << 8) | u64::from(*byte));
    let mut value = format!("S-{}-{authority}", bytes[0]);
    for index in 0..sub_authorities {
        let offset = 8 + index * 4;
        let part = u32::from_le_bytes(bytes[offset..offset + 4].try_into()?);
        value.push_str(&format!("-{part}"));
    }
    Ok(value)
}

fn task_name() -> Result<String, Box<dyn std::error::Error>> {
    Ok(task_name_for_scope(&llm_meter_ipc::user_scope_id()?))
}

fn task_name_for_scope(scope: &str) -> String {
    format!("{TASK_PREFIX} - {scope}")
}

fn daemon_path() -> Result<PathBuf, Box<dyn std::error::Error>> {
    let sibling = std::env::current_exe().ok().and_then(|executable| {
        let daemon = executable.parent()?.join("llm-meterd.exe");
        daemon.is_file().then_some(daemon)
    });
    let daemon = sibling
        .or_else(|| find_command("llm-meterd.exe"))
        .ok_or("llm-meterd.exe was not found next to the CLI or on PATH")?;
    Ok(daemon.canonicalize().unwrap_or(daemon))
}

fn find_command(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH")?
        .to_string_lossy()
        .split(';')
        .filter(|entry| !entry.is_empty())
        .map(|entry| PathBuf::from(entry.trim_matches('"')).join(name))
        .find(|candidate| candidate.is_file())
}

fn windows_paths_equal(actual: &str, expected: &Path) -> bool {
    normalize_windows_path(actual) == normalize_windows_path(&expected.to_string_lossy())
}

fn normalize_windows_path(value: &str) -> String {
    value
        .strip_prefix(r"\\?\")
        .unwrap_or(value)
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_lowercase()
}

fn with_task_service<T>(
    operation: impl FnOnce(
        &ITaskService,
        &ITaskFolder,
        &UserIdentity,
    ) -> Result<T, Box<dyn std::error::Error>>,
) -> Result<T, Box<dyn std::error::Error>> {
    let _apartment = ComApartment::initialize()?;
    let service: ITaskService =
        unsafe { CoCreateInstance(&TaskScheduler, None, CLSCTX_INPROC_SERVER)? };
    unsafe {
        service.Connect(
            &VARIANT::default(),
            &VARIANT::default(),
            &VARIANT::default(),
            &VARIANT::default(),
        )?;
    }
    let folder = unsafe { service.GetFolder(&BSTR::from("\\"))? };
    let identity = current_user_identity()?;
    operation(&service, &folder, &identity)
}

struct ComApartment;

impl ComApartment {
    fn initialize() -> windows::core::Result<Self> {
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
        }
        Ok(Self)
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        unsafe {
            CoUninitialize();
        }
    }
}

fn confirm(prompt: &str, default: bool) -> io::Result<bool> {
    let suffix = if default { " [Y/n]: " } else { " [y/N]: " };
    print!("{prompt}{suffix}");
    io::stdout().flush()?;
    let mut answer = String::new();
    if io::stdin().read_line(&mut answer)? == 0 {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "interactive input ended; rerun with --non-interactive",
        ));
    }
    Ok(match answer.trim().to_ascii_lowercase().as_str() {
        "" => default,
        "y" | "yes" => true,
        _ => false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_binary_sid_without_locale_sensitive_commands() {
        let sid = [1, 2, 0, 0, 0, 0, 0, 5, 32, 0, 0, 0, 33, 2, 0, 0];
        assert_eq!(sid_string(&sid).unwrap(), "S-1-5-32-545");
    }

    #[test]
    fn task_name_is_stable_and_does_not_disclose_the_sid() {
        assert_eq!(
            task_name_for_scope("567890abcdef"),
            "LLM Meter Daemon - 567890abcdef"
        );
    }

    #[test]
    fn compares_windows_paths_case_insensitively() {
        assert!(windows_paths_equal(
            r"C:\Users\Example\LLM Meter\llm-meterd.exe",
            Path::new(r"c:/users/example/LLM Meter/llm-meterd.exe"),
        ));
    }

    #[test]
    fn recognizes_only_missing_task_hresults() {
        assert!(task_missing_code(0x8007_0002));
        assert!(task_missing_code(0x8007_0003));
        assert!(!task_missing_code(0x8004_130f));
        assert!(!task_missing_code(0x8007_0005));
    }
}
