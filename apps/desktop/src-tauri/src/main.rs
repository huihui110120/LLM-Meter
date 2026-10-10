#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use llm_meter_ipc::{call, endpoint};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    fs,
    path::PathBuf,
    process::{Command, Stdio},
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tauri::{
    AppHandle, LogicalSize, Manager, PhysicalPosition, PhysicalSize, WebviewUrl,
    WebviewWindowBuilder, WindowEvent,
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
};

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

const DAEMON_START_TIMEOUT: Duration = Duration::from_secs(5);
const DAEMON_PROBE_INTERVAL: Duration = Duration::from_millis(100);
const DAEMON_PROBE_TIMEOUT: Duration = Duration::from_secs(2);
const TRAY_ID: &str = "llm-meter";
const WIDGET_WIDTH: f64 = 320.0;
const WIDGET_HEIGHT: f64 = 48.0;
const WIDGET_EXPANDED_WIDTH: f64 = 420.0;
const WIDGET_EXPANDED_HEIGHT: f64 = 480.0;
const WIDGET_MARGIN: i32 = 20;
const WIDGET_VISIBLE_EDGE: i32 = 32;
#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

static DAEMON_START_LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
static WIDGET_EXPANDED: AtomicBool = AtomicBool::new(false);
static WIDGET_COMPACT_POSITION: OnceLock<Mutex<Option<PhysicalPosition<i32>>>> = OnceLock::new();

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
struct WidgetPosition {
    x: i32,
    y: i32,
}

async fn daemon_call(method: &str, params: Value) -> Result<Value, String> {
    ensure_daemon().await?;
    let ipc_endpoint = endpoint().map_err(|error| error.to_string())?;
    call(&ipc_endpoint, method, params)
        .await
        .map_err(|error| error.to_string())
}

async fn ensure_daemon() -> Result<(), String> {
    let ipc_endpoint = endpoint().map_err(|error| error.to_string())?;
    match probe_daemon(&ipc_endpoint, DAEMON_PROBE_TIMEOUT).await {
        Ok(_) => return Ok(()),
        Err(error) if incompatible_daemon(&error.to_string()) => {
            return Err(error.to_string());
        }
        Err(_) => {}
    }

    let lock = DAEMON_START_LOCK.get_or_init(|| tokio::sync::Mutex::new(()));
    let _guard = lock.lock().await;

    // Another command may have started the daemon while this command waited.
    let ipc_endpoint = endpoint().map_err(|error| error.to_string())?;
    match probe_daemon(&ipc_endpoint, DAEMON_PROBE_TIMEOUT).await {
        Ok(_) => return Ok(()),
        Err(error) if incompatible_daemon(&error.to_string()) => {
            return Err(error.to_string());
        }
        Err(_) => {}
    }

    start_sibling_daemon()?;
    let deadline = Instant::now() + DAEMON_START_TIMEOUT;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(format!(
                "llm-meterd did not become ready within {} seconds",
                DAEMON_START_TIMEOUT.as_secs()
            ));
        }
        let ipc_endpoint = endpoint().map_err(|error| error.to_string())?;
        match probe_daemon(&ipc_endpoint, remaining.min(DAEMON_PROBE_TIMEOUT)).await {
            Ok(_) => return Ok(()),
            Err(error) if Instant::now() >= deadline => {
                return Err(format!(
                    "llm-meterd did not become ready within {} seconds: {error}",
                    DAEMON_START_TIMEOUT.as_secs()
                ));
            }
            Err(_) => {}
        }
        tokio::time::sleep(DAEMON_PROBE_INTERVAL).await;
    }
}

async fn probe_daemon(
    endpoint: &llm_meter_ipc::IpcEndpoint,
    timeout: Duration,
) -> Result<Value, String> {
    tokio::time::timeout(timeout, call(endpoint, "system/version", json!({})))
        .await
        .map_err(|_| "daemon IPC probe timed out".to_owned())?
        .map_err(|error| error.to_string())
}

fn incompatible_daemon(error: &str) -> bool {
    error.contains("incompatible daemon IPC version")
}

fn start_sibling_daemon() -> Result<(), String> {
    let executable_name = format!("llm-meterd{}", std::env::consts::EXE_SUFFIX);
    let sibling = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|parent| parent.join(&executable_name)))
        .filter(|path| path.is_file());
    let program = match sibling {
        Some(path) => path,
        None if cfg!(debug_assertions) => executable_name.into(),
        None => {
            return Err(format!(
                "{executable_name} is missing from the desktop installation directory"
            ));
        }
    };
    let mut command = Command::new(&program);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(target_os = "windows")]
    command.creation_flags(CREATE_NO_WINDOW);
    command
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("could not start {}: {error}", program.display()))
}

#[tauri::command]
async fn snapshot() -> Result<Value, String> {
    daemon_call("snapshot/get", json!({})).await
}

#[tauri::command]
async fn provider_catalog() -> Result<Value, String> {
    daemon_call("providers/list", json!({})).await
}

#[tauri::command]
async fn refresh_all() -> Result<Value, String> {
    daemon_call("connections/refresh-all", json!({})).await
}

#[tauri::command]
async fn set_budget(
    budget_id: String,
    connection_id: String,
    amount: String,
    currency: String,
) -> Result<Value, String> {
    daemon_call(
        "budgets/set",
        json!({
            "id": budget_id,
            "connection_id": connection_id,
            "amount": amount,
            "currency": currency.to_uppercase(),
            "period": "monthly",
            "warning_ratio": "0.8",
            "critical_ratio": "1",
            "enabled": true,
        }),
    )
    .await
}

#[tauri::command]
async fn refresh_provider(connection_id: String) -> Result<Value, String> {
    daemon_call(
        "connections/refresh",
        json!({"connection_id": connection_id}),
    )
    .await
}

#[tauri::command]
async fn remove_connection(connection_id: String) -> Result<Value, String> {
    daemon_call(
        "connections/remove",
        json!({"connection_id": connection_id}),
    )
    .await
}

#[tauri::command]
async fn begin_auth(
    provider_id: String,
    connection_type: String,
    display_name: String,
    auth_scheme: String,
    settings: Option<Value>,
) -> Result<Value, String> {
    daemon_call(
        "connections/add",
        json!({
            "provider_id": provider_id,
            "connection_type": connection_type,
            "display_name": display_name,
            "auth_scheme": auth_scheme,
            "settings": settings.unwrap_or_else(|| json!({"schema_version":0,"values":{}})),
        }),
    )
    .await
}

#[tauri::command]
async fn validate_compatible(origin: String, secret: String) -> Result<Value, String> {
    let result = daemon_call(
        "connections/validate-compatible",
        json!({"origin":origin,"secret":secret}),
    )
    .await;
    drop(secret);
    result
}

#[tauri::command]
async fn proxy_action(
    method: String,
    connection_id: String,
    display_name: Option<String>,
    credential_id: Option<String>,
    token: Option<String>,
) -> Result<Value, String> {
    if !matches!(
        method.as_str(),
        "proxy/status"
            | "proxy/start"
            | "proxy/stop"
            | "proxy/credentials/list"
            | "proxy/credentials/create"
            | "proxy/credentials/disable"
    ) {
        return Err("unsupported proxy action".into());
    }
    let result = daemon_call(
        &method,
        json!({
            "connection_id": connection_id,
            "display_name": display_name,
            "credential_id": credential_id,
            "token": token,
        }),
    )
    .await;
    drop(token);
    result
}

#[tauri::command]
async fn complete_auth(challenge_id: String, secret: Option<String>) -> Result<Value, String> {
    let result = daemon_call(
        "connections/auth/complete",
        json!({"challenge_id": challenge_id, "secret": secret.as_deref()}),
    )
    .await;
    drop(secret);
    result
}

#[tauri::command]
fn open_auth_url(auth_url: String) -> Result<(), String> {
    let parsed = url::Url::parse(&auth_url).map_err(|_| "认证地址无效".to_owned())?;
    if parsed.scheme() != "https" {
        return Err("只允许打开 HTTPS 认证地址".into());
    }
    open_external(parsed.as_str())
}

#[tauri::command]
fn open_release() -> Result<(), String> {
    open_external("https://github.com/huihui110120/LLM-Meter/releases/latest")
}

#[tauri::command]
fn update_tray_tooltip(app: AppHandle, tooltip: String) -> Result<(), String> {
    let tooltip = normalized_tray_tooltip(&tooltip);
    let tray = app
        .tray_by_id(TRAY_ID)
        .ok_or_else(|| "Windows tray icon is unavailable".to_owned())?;
    tray.set_tooltip(Some(tooltip))
        .map_err(|error| error.to_string())
}

fn normalized_tray_tooltip(value: &str) -> String {
    let sanitized = value.replace('\0', " ");
    let value = sanitized.trim();
    if value.is_empty() {
        return "LLM Meter".to_owned();
    }
    value.chars().take(240).collect()
}

#[tauri::command]
async fn autostart_action(action: String) -> Result<Value, String> {
    if !matches!(action.as_str(), "status" | "enable" | "disable") {
        return Err("unsupported autostart action".into());
    }
    tokio::task::spawn_blocking(move || run_sibling_cli(&["autostart", &action]))
        .await
        .map_err(|error| format!("autostart task failed: {error}"))?
}

fn run_sibling_cli(args: &[&str]) -> Result<Value, String> {
    let executable_name = format!("llm-meter{}", std::env::consts::EXE_SUFFIX);
    let program = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|parent| parent.join(&executable_name)))
        .filter(|path| path.is_file())
        .ok_or_else(|| {
            format!("{executable_name} is missing from the desktop installation directory")
        })?;
    let mut command = Command::new(&program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(target_os = "windows")]
    command.creation_flags(CREATE_NO_WINDOW);
    let output = command
        .output()
        .map_err(|error| format!("could not run {}: {error}", program.display()))?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        return Err(if detail.is_empty() {
            format!("{} exited with {}", program.display(), output.status)
        } else {
            detail
        });
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("invalid response from {}: {error}", program.display()))
}

fn open_external(url: &str) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    let program = std::env::var_os("WINDIR")
        .or_else(|| std::env::var_os("SystemRoot"))
        .map(std::path::PathBuf::from)
        .map(|root| root.join("System32").join("rundll32.exe"))
        .unwrap_or_else(|| std::path::PathBuf::from("rundll32.exe"));
    #[cfg(target_os = "macos")]
    let program = std::path::PathBuf::from("open");
    #[cfg(all(unix, not(target_os = "macos")))]
    let program = std::path::PathBuf::from("xdg-open");
    #[cfg(not(any(target_os = "windows", target_os = "macos", unix)))]
    return Err("当前平台不支持打开系统浏览器".into());

    let mut command = Command::new(&program);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(target_os = "windows")]
    command.arg("url.dll,FileProtocolHandler").arg(url);
    #[cfg(not(target_os = "windows"))]
    command.arg(url);
    #[cfg(target_os = "windows")]
    command.creation_flags(CREATE_NO_WINDOW);
    command
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("无法打开系统浏览器: {error}"))
}

#[tauri::command]
fn close_surface(window: tauri::WebviewWindow) -> Result<(), String> {
    if window.label() == "widget" {
        set_widget_expanded_layout(&window, false)
    } else {
        window.close().map_err(|e| e.to_string())
    }
}

#[tauri::command]
fn drag_surface(window: tauri::WebviewWindow) -> Result<(), String> {
    window.start_dragging().map_err(|error| error.to_string())
}

#[tauri::command]
fn activate(app: AppHandle, action: String) -> Result<(), String> {
    apply_action(&app, &action)
}

fn widget_position_path(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_config_dir()
        .map(|directory| directory.join("widget-position.json"))
        .map_err(|error| error.to_string())
}

fn load_widget_position(app: &AppHandle) -> Option<PhysicalPosition<i32>> {
    let path = widget_position_path(app).ok()?;
    let value: WidgetPosition = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
    Some(PhysicalPosition::new(value.x, value.y))
}

fn save_widget_position(app: &AppHandle, position: PhysicalPosition<i32>) -> Result<(), String> {
    let path = widget_position_path(app)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let value = WidgetPosition {
        x: position.x,
        y: position.y,
    };
    let json = serde_json::to_vec(&value).map_err(|error| error.to_string())?;
    fs::write(path, json).map_err(|error| error.to_string())
}

fn remember_widget_position(
    app: &AppHandle,
    position: PhysicalPosition<i32>,
) -> Result<(), String> {
    let mut stored = WIDGET_COMPACT_POSITION
        .get_or_init(|| Mutex::new(None))
        .lock()
        .map_err(|_| "widget position state is unavailable".to_owned())?;
    *stored = Some(position);
    drop(stored);
    save_widget_position(app, position)
}

fn remembered_widget_position(app: &AppHandle) -> Option<PhysicalPosition<i32>> {
    WIDGET_COMPACT_POSITION
        .get_or_init(|| Mutex::new(None))
        .lock()
        .ok()
        .and_then(|stored| *stored)
        .or_else(|| load_widget_position(app))
}

fn widget_position_is_visible(app: &AppHandle, position: PhysicalPosition<i32>) -> bool {
    app.available_monitors()
        .map(|monitors| {
            monitors.iter().any(|monitor| {
                let origin = monitor.position();
                let size = monitor.size();
                let right = origin.x.saturating_add(size.width as i32);
                let bottom = origin.y.saturating_add(size.height as i32);
                position.x.saturating_add(WIDGET_VISIBLE_EDGE) > origin.x
                    && position.y.saturating_add(WIDGET_VISIBLE_EDGE) > origin.y
                    && position.x < right
                    && position.y < bottom
            })
        })
        .unwrap_or(false)
}

fn place_widget(window: &tauri::WebviewWindow) -> Result<(), String> {
    let app = window.app_handle();
    if let Some(position) = load_widget_position(app)
        && widget_position_is_visible(app, position)
    {
        window
            .set_position(position)
            .map_err(|error| error.to_string())?;
        return remember_widget_position(app, position);
    }

    let Some(monitor) = app.primary_monitor().map_err(|error| error.to_string())? else {
        return Ok(());
    };
    let origin = monitor.position();
    let monitor_size = monitor.size();
    let window_size = window.outer_size().map_err(|error| error.to_string())?;
    let x = origin
        .x
        .saturating_add(monitor_size.width as i32)
        .saturating_sub(window_size.width as i32)
        .saturating_sub(WIDGET_MARGIN);
    let y = origin.y.saturating_add(WIDGET_MARGIN);
    let position = PhysicalPosition::new(x, y);
    window
        .set_position(position)
        .map_err(|error| error.to_string())?;
    remember_widget_position(app, position)
}

fn widget(app: &AppHandle) -> Result<tauri::WebviewWindow, String> {
    let window = WebviewWindowBuilder::new(
        app,
        "widget",
        WebviewUrl::App("index.html?role=widget".into()),
    )
    .title("LLM Meter Widget")
    .inner_size(WIDGET_WIDTH, WIDGET_HEIGHT)
    .decorations(false)
    .resizable(false)
    .always_on_top(true)
    .skip_taskbar(true)
    .transparent(true)
    .build()
    .map_err(|error| error.to_string())?;
    place_widget(&window)?;
    Ok(window)
}

fn show_widget(app: &AppHandle, take_focus: bool) -> Result<tauri::WebviewWindow, String> {
    let window = match app.get_webview_window("widget") {
        Some(window) => window,
        None => widget(app)?,
    };
    window.show().map_err(|error| error.to_string())?;
    if take_focus {
        window.set_focus().map_err(|error| error.to_string())?;
    }
    Ok(window)
}

fn anchored_widget_position(
    right_edge: i32,
    top: i32,
    target_size: PhysicalSize<u32>,
    monitor_position: PhysicalPosition<i32>,
    monitor_size: PhysicalSize<u32>,
) -> PhysicalPosition<i32> {
    let monitor_right = monitor_position.x.saturating_add(monitor_size.width as i32);
    let monitor_bottom = monitor_position
        .y
        .saturating_add(monitor_size.height as i32);
    let maximum_x = monitor_right
        .saturating_sub(target_size.width as i32)
        .max(monitor_position.x);
    let maximum_y = monitor_bottom
        .saturating_sub(target_size.height as i32)
        .max(monitor_position.y);
    let x = right_edge
        .saturating_sub(target_size.width as i32)
        .clamp(monitor_position.x, maximum_x);
    let y = top.clamp(monitor_position.y, maximum_y);
    PhysicalPosition::new(x, y)
}

fn widget_target_size(
    window: &tauri::WebviewWindow,
    expanded: bool,
) -> Result<(LogicalSize<f64>, PhysicalSize<u32>), String> {
    let logical = if expanded {
        LogicalSize::new(WIDGET_EXPANDED_WIDTH, WIDGET_EXPANDED_HEIGHT)
    } else {
        LogicalSize::new(WIDGET_WIDTH, WIDGET_HEIGHT)
    };
    let scale_factor = window.scale_factor().map_err(|error| error.to_string())?;
    Ok((logical, logical.to_physical(scale_factor)))
}

fn sync_widget_expanded(window: &tauri::WebviewWindow, expanded: bool) -> Result<(), String> {
    let value = if expanded { "true" } else { "false" };
    window
        .eval(format!(
            "document.body.dataset.expanded = '{value}'; window.__llmMeterSetWidgetExpanded?.({value});"
        ))
        .map_err(|error| error.to_string())
}

fn set_widget_expanded_layout(window: &tauri::WebviewWindow, expanded: bool) -> Result<(), String> {
    if window.label() != "widget" {
        return Err("only the widget surface can be expanded".into());
    }
    if WIDGET_EXPANDED.load(Ordering::SeqCst) == expanded {
        return sync_widget_expanded(window, expanded);
    }

    let app = window.app_handle();
    let current_position = window.outer_position().map_err(|error| error.to_string())?;
    let current_size = window.outer_size().map_err(|error| error.to_string())?;
    let (target_logical_size, target_physical_size) = widget_target_size(window, expanded)?;
    let monitor = window
        .current_monitor()
        .map_err(|error| error.to_string())?
        .or(app.primary_monitor().map_err(|error| error.to_string())?);

    if expanded {
        remember_widget_position(app, current_position)?;
        let right_edge = current_position.x.saturating_add(current_size.width as i32);
        let target_position = monitor
            .as_ref()
            .map(|monitor| {
                anchored_widget_position(
                    right_edge,
                    current_position.y,
                    target_physical_size,
                    *monitor.position(),
                    *monitor.size(),
                )
            })
            .unwrap_or(current_position);

        WIDGET_EXPANDED.store(true, Ordering::SeqCst);
        if let Err(error) = window.set_size(target_logical_size) {
            WIDGET_EXPANDED.store(false, Ordering::SeqCst);
            return Err(error.to_string());
        }
        if let Err(error) = window.set_position(target_position) {
            let _ = window.set_size(LogicalSize::new(WIDGET_WIDTH, WIDGET_HEIGHT));
            let _ = window.set_position(current_position);
            WIDGET_EXPANDED.store(false, Ordering::SeqCst);
            return Err(error.to_string());
        }
        sync_widget_expanded(window, true)
    } else {
        let compact_position = remembered_widget_position(app).unwrap_or_else(|| {
            PhysicalPosition::new(
                current_position
                    .x
                    .saturating_add(current_size.width as i32)
                    .saturating_sub(target_physical_size.width as i32),
                current_position.y,
            )
        });
        let compact_right = compact_position
            .x
            .saturating_add(target_physical_size.width as i32);
        let target_position = monitor
            .as_ref()
            .map(|monitor| {
                anchored_widget_position(
                    compact_right,
                    compact_position.y,
                    target_physical_size,
                    *monitor.position(),
                    *monitor.size(),
                )
            })
            .unwrap_or(compact_position);

        window
            .set_size(target_logical_size)
            .map_err(|error| error.to_string())?;
        window
            .set_position(target_position)
            .map_err(|error| error.to_string())?;
        remember_widget_position(app, target_position)?;
        WIDGET_EXPANDED.store(false, Ordering::SeqCst);
        sync_widget_expanded(window, false)
    }
}

#[tauri::command]
fn set_widget_expanded(window: tauri::WebviewWindow, expanded: bool) -> Result<(), String> {
    set_widget_expanded_layout(&window, expanded)
}

#[tauri::command]
fn widget_expanded(window: tauri::WebviewWindow) -> Result<bool, String> {
    if window.label() != "widget" {
        return Err("only the widget surface has an expanded state".into());
    }
    Ok(WIDGET_EXPANDED.load(Ordering::SeqCst))
}

fn main_window(app: &AppHandle) -> Result<tauri::WebviewWindow, String> {
    WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html?role=main".into()))
        .title("LLM Meter")
        .inner_size(900.0, 720.0)
        .min_inner_size(380.0, 500.0)
        .decorations(true)
        .resizable(true)
        .build()
        .map_err(|e| e.to_string())
}

fn focus(window: &tauri::WebviewWindow) -> Result<(), String> {
    window.show().map_err(|e| e.to_string())?;
    window.set_focus().map_err(|e| e.to_string())
}

fn apply_action(app: &AppHandle, action: &str) -> Result<(), String> {
    match action {
        "hide" => {
            if let Some(window) = app.get_webview_window("widget") {
                if WIDGET_EXPANDED.load(Ordering::SeqCst) {
                    let _ = set_widget_expanded_layout(&window, false);
                }
                window.hide().map_err(|error| error.to_string())?;
            }
        }
        "collapse" => {
            if let Some(window) = app.get_webview_window("widget")
                && WIDGET_EXPANDED.load(Ordering::SeqCst)
            {
                set_widget_expanded_layout(&window, false)?;
            }
        }
        "main" => {
            if let Some(window) = app.get_webview_window("widget")
                && WIDGET_EXPANDED.load(Ordering::SeqCst)
            {
                let _ = set_widget_expanded_layout(&window, false);
            }
            match app.get_webview_window("main") {
                Some(window) => focus(&window)?,
                None => {
                    main_window(app)?;
                }
            }
        }
        "toggle" => {
            let window = show_widget(app, false)?;
            let expanded = !WIDGET_EXPANDED.load(Ordering::SeqCst);
            set_widget_expanded_layout(&window, expanded)?;
        }
        _ => {
            show_widget(app, true)?;
        }
    }
    Ok(())
}

fn requested_action(args: &[String]) -> &'static str {
    if args.iter().any(|v| v == "--main") {
        "main"
    } else if args.iter().any(|v| v == "--hide") {
        "hide"
    } else if args.iter().any(|v| v == "--toggle") {
        "toggle"
    } else {
        "show"
    }
}

fn build_tray(app: &tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let show = MenuItem::with_id(app, "show", "显示迷你组件", true, None::<&str>)?;
    let main = MenuItem::with_id(app, "main", "打开 LLM Meter", true, None::<&str>)?;
    let refresh = MenuItem::with_id(app, "refresh", "立即刷新", true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, "quit", "退出桌面端", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &main, &refresh, &separator, &quit])?;

    TrayIconBuilder::with_id(TRAY_ID)
        .icon(
            app.default_window_icon()
                .cloned()
                .ok_or("application icon missing")?,
        )
        .tooltip("LLM Meter")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => {
                let _ = apply_action(app, "show");
            }
            "main" => {
                let _ = apply_action(app, "main");
            }
            "refresh" => {
                tauri::async_runtime::spawn(async {
                    let _ = daemon_call("connections/refresh-all", json!({})).await;
                });
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if matches!(
                event,
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                }
            ) {
                let _ = apply_action(tray.app_handle(), "toggle");
            }
        })
        .build(app)?;
    Ok(())
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            let _ = apply_action(app, requested_action(&args));
        }))
        .invoke_handler(tauri::generate_handler![
            snapshot,
            provider_catalog,
            refresh_all,
            set_budget,
            refresh_provider,
            remove_connection,
            begin_auth,
            complete_auth,
            validate_compatible,
            proxy_action,
            open_auth_url,
            open_release,
            update_tray_tooltip,
            autostart_action,
            close_surface,
            drag_surface,
            activate,
            set_widget_expanded,
            widget_expanded
        ])
        .on_window_event(|window, event| match event {
            WindowEvent::Moved(position)
                if window.label() == "widget" && !WIDGET_EXPANDED.load(Ordering::SeqCst) =>
            {
                let _ = remember_widget_position(window.app_handle(), *position);
            }
            WindowEvent::CloseRequested { api, .. } if window.label() == "widget" => {
                api.prevent_close();
                if WIDGET_EXPANDED.load(Ordering::SeqCst)
                    && let Some(widget) = window.app_handle().get_webview_window("widget")
                {
                    let _ = set_widget_expanded_layout(&widget, false);
                }
                let _ = window.hide();
            }
            _ => {}
        })
        .setup(|app| {
            tauri::async_runtime::spawn(async {
                let _ = ensure_daemon().await;
            });
            let action = requested_action(&std::env::args().collect::<Vec<_>>());
            apply_action(app.handle(), action).map_err(std::io::Error::other)?;
            if std::env::var_os("LLM_METER_NO_TRAY").is_none() {
                build_tray(app)?;
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("LLM Meter desktop runtime failed");
}

#[cfg(test)]
mod tests {
    use super::{anchored_widget_position, normalized_tray_tooltip, requested_action};
    use tauri::{PhysicalPosition, PhysicalSize};

    #[test]
    fn hide_takes_precedence_over_toggle_for_autostart() {
        let args = vec![
            "llm-meter-desktop".into(),
            "--toggle".into(),
            "--hide".into(),
        ];
        assert_eq!(requested_action(&args), "hide");
    }

    #[test]
    fn direct_launch_shows_the_widget() {
        assert_eq!(requested_action(&["llm-meter-desktop".into()]), "show");
    }

    #[test]
    fn tray_tooltip_is_non_empty_and_bounded() {
        assert_eq!(normalized_tray_tooltip(" \0 "), "LLM Meter");
        assert_eq!(
            normalized_tray_tooltip(&"x".repeat(300)).chars().count(),
            240
        );
    }

    #[test]
    fn expanded_widget_keeps_the_compact_right_edge_and_top() {
        let position = anchored_widget_position(
            1900,
            20,
            (420, 480).into(),
            (0, 0).into(),
            (1920, 1080).into(),
        );
        assert_eq!(position, PhysicalPosition::new(1480, 20));
    }

    #[test]
    fn expanded_widget_is_clamped_inside_its_monitor() {
        let position = anchored_widget_position(
            2500,
            900,
            (420, 480).into(),
            (1920, 0).into(),
            PhysicalSize::new(1280, 1024),
        );
        assert_eq!(position, PhysicalPosition::new(2080, 544));
    }
}
