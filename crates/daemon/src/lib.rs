pub mod alerts;
pub mod config;
pub mod ipc;
pub mod local_codex;
pub mod notifications;
mod runner;
pub mod scheduler;
pub mod snapshot;
pub mod telemetry;

pub use llm_meter_ipc::{IpcEndpoint, endpoint as ipc_endpoint};
pub use runner::run;

use std::{io, path::PathBuf};

/// Persistent application root. Secrets stay in the operating-system keyring.
pub fn app_home() -> Result<PathBuf, io::Error> {
    if let Some(path) = absolute_env_path("LLM_METER_HOME")? {
        return Ok(path);
    }
    default_app_home()
}

#[cfg(unix)]
fn default_app_home() -> Result<PathBuf, io::Error> {
    absolute_env_path("HOME")?
        .map(|home| home.join(".llm-meter"))
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME is required"))
}

#[cfg(windows)]
fn default_app_home() -> Result<PathBuf, io::Error> {
    if let Some(local) = absolute_env_path("LOCALAPPDATA")? {
        return Ok(local.join("LLM Meter"));
    }
    absolute_env_path("USERPROFILE")?
        .map(|profile| profile.join("AppData").join("Local").join("LLM Meter"))
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "LOCALAPPDATA or USERPROFILE is required",
            )
        })
}

#[cfg(not(any(unix, windows)))]
fn default_app_home() -> Result<PathBuf, io::Error> {
    absolute_env_path("HOME")?
        .map(|home| home.join(".llm-meter"))
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME is required"))
}

pub fn codex_home() -> Result<PathBuf, io::Error> {
    if let Some(path) = absolute_env_path("CODEX_HOME")? {
        return Ok(path);
    }
    default_codex_home()
}

#[cfg(windows)]
fn default_codex_home() -> Result<PathBuf, io::Error> {
    absolute_env_path("USERPROFILE")?
        .map(|profile| profile.join(".codex"))
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "USERPROFILE is required"))
}

#[cfg(not(windows))]
fn default_codex_home() -> Result<PathBuf, io::Error> {
    absolute_env_path("HOME")?
        .map(|home| home.join(".codex"))
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME is required"))
}

#[cfg(unix)]
pub fn runtime_dir() -> Result<PathBuf, io::Error> {
    absolute_env_path("XDG_RUNTIME_DIR")?
        .map(|base| base.join("llm-meter"))
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "XDG_RUNTIME_DIR is required; refusing an unsafe /tmp fallback",
            )
        })
}

#[cfg(windows)]
pub fn runtime_dir() -> Result<PathBuf, io::Error> {
    Ok(app_home()?.join("runtime"))
}

#[cfg(not(any(unix, windows)))]
pub fn runtime_dir() -> Result<PathBuf, io::Error> {
    Ok(app_home()?.join("runtime"))
}

#[cfg(unix)]
pub fn socket_path() -> Result<PathBuf, io::Error> {
    Ok(llm_meter_ipc::endpoint()?.as_path().to_owned())
}

fn absolute_env_path(name: &str) -> Result<Option<PathBuf>, io::Error> {
    std::env::var_os(name)
        .map(PathBuf::from)
        .map(|path| validate_absolute_path(name, path))
        .transpose()
}

fn validate_absolute_path(name: &str, path: PathBuf) -> Result<PathBuf, io::Error> {
    if path.is_absolute() {
        Ok(path)
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{name} must be an absolute path"),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_overrides_must_be_absolute() {
        let absolute = std::env::current_dir().unwrap().join("llm-meter-test");
        assert_eq!(
            validate_absolute_path("LLM_METER_HOME", absolute.clone()).unwrap(),
            absolute
        );
        let error =
            validate_absolute_path("LLM_METER_HOME", PathBuf::from("relative")).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(error.to_string().contains("LLM_METER_HOME"));
    }
}
