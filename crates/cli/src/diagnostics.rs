use llm_meter_ipc::{IpcEndpoint, call, endpoint};
use llm_meter_secret_store::NativeSecretStore;
use serde_json::{Value, json};

pub async fn collect() -> Result<Value, Box<dyn std::error::Error>> {
    let endpoint = endpoint()?;
    let version = call(&endpoint, "system/version", json!({})).await;
    let health = call(&endpoint, "system/health", json!({})).await;
    let providers = call(&endpoint, "providers/list", json!({})).await;
    let connections = call(&endpoint, "connections/list", json!({})).await;
    let secret_store = NativeSecretStore.availability().await;
    let autostart = crate::setup::autostart_status();

    let daemon_reachable = version.is_ok();
    let connection_count = connections
        .as_ref()
        .ok()
        .and_then(Value::as_array)
        .map(Vec::len);
    let mut hints = platform_hints(secret_store, daemon_reachable);
    let autostart_value = match autostart {
        Ok(status) => serde_json::to_value(status)?,
        Err(error) => {
            hints.push(format!("Could not inspect daemon autostart: {error}"));
            json!({"error":"autostart-status-unavailable"})
        }
    };

    Ok(json!({
        "diagnostics_schema": 1,
        "os": std::env::consts::OS,
        "architecture": std::env::consts::ARCH,
        "version": version.ok(),
        "health": health.ok(),
        "providers": providers.ok(),
        "connection_count": connection_count,
        "daemon": {
            "reachable": daemon_reachable,
            "autostart": autostart_value,
        },
        "secret_store": {
            "backend": secret_backend_name(),
            "state": secret_store,
        },
        "platform": platform_details(&endpoint),
        "window_identity": {"expected":"io.github.llmmeter"},
        "hints": hints,
    }))
}

#[cfg(unix)]
fn platform_details(endpoint: &IpcEndpoint) -> Value {
    use std::os::unix::fs::FileTypeExt;

    let socket = match std::fs::metadata(endpoint.as_path()) {
        Ok(metadata) if metadata.file_type().is_socket() => "available",
        Ok(_) => "not-a-socket",
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => "permission-denied",
        Err(_) => "missing",
    };
    let user_bus_names =
        command_line("busctl", &["--user", "--no-pager", "--list"]).unwrap_or_default();
    let notifications = user_bus_names.contains("org.freedesktop.Notifications");
    let portal = user_bus_names.contains("org.freedesktop.portal.Desktop");

    json!({
        "session": {
            "type": std::env::var("XDG_SESSION_TYPE").unwrap_or_else(|_| "unknown".into()),
            "desktop": std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_else(|_| "unknown".into()),
            "wayland_display": std::env::var_os("WAYLAND_DISPLAY").is_some(),
            "dbus": std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_some(),
        },
        "ipc": {"transport":"unix-socket","state":socket},
        "hyprland": {
            "present": std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some(),
            "version": command_line("hyprctl", &["version"])
                .and_then(|value| value.lines().next().map(str::to_owned)),
        },
        "notification_service": {"available":notifications},
        "portal": {
            "desktop":portal,
            "hyprland_service":command_success(
                "systemctl",
                &["--user","is-active","xdg-desktop-portal-hyprland.service"],
            ),
        },
        "webkitgtk": {"version":command_line("pkg-config", &["--modversion","webkit2gtk-4.1"])},
    })
}

#[cfg(windows)]
fn platform_details(endpoint: &IpcEndpoint) -> Value {
    json!({
        "session": {
            "interactive": std::env::var_os("SESSIONNAME").is_some(),
        },
        "ipc": {
            "transport":"named-pipe",
            "scope":"current-user",
            "configured": !endpoint.pipe_name().is_empty(),
        },
        "install_scope":"current-user",
    })
}

#[cfg(unix)]
fn platform_hints(secret_store: &str, daemon_reachable: bool) -> Vec<String> {
    let mut hints = Vec::new();
    if !daemon_reachable {
        hints.push("Enable or start the llm-meterd systemd user service.".into());
    }
    if std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_none() {
        hints.push(
            "Import the graphical session D-Bus environment (UWSM is recommended on Hyprland)."
                .into(),
        );
    }
    if secret_store != "available" {
        hints.push(
            "Start and unlock a Secret Service provider such as GNOME Keyring or KeePassXC.".into(),
        );
    }
    hints
}

#[cfg(windows)]
fn platform_hints(secret_store: &str, daemon_reachable: bool) -> Vec<String> {
    let mut hints = Vec::new();
    if !daemon_reachable {
        hints.push("Run `llm-meter autostart enable` to repair and start the daemon task.".into());
    }
    if secret_store != "available" {
        hints.push("Windows Credential Manager is unavailable for the current user.".into());
    }
    hints
}

#[cfg(unix)]
fn secret_backend_name() -> &'static str {
    "secret-service"
}

#[cfg(windows)]
fn secret_backend_name() -> &'static str {
    "credential-manager"
}

#[cfg(unix)]
fn command_line(program: &str, args: &[&str]) -> Option<String> {
    let output = std::process::Command::new(program)
        .args(args)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .filter(|value| !value.is_empty())
}

#[cfg(unix)]
fn command_success(program: &str, args: &[&str]) -> bool {
    std::process::Command::new(program)
        .args(args)
        .output()
        .is_ok_and(|output| output.status.success())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostic_schema_does_not_embed_connection_records() {
        let value = json!({
            "connection_count": 2,
            "secret_store": {"state":"available"},
        });
        assert!(value.get("connections").is_none());
        assert_eq!(value["connection_count"], 2);
    }
}
