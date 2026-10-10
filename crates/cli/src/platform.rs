use std::{path::PathBuf, process::Command};

const MAX_EXTERNAL_URL_LENGTH: usize = 8 * 1024;

/// Open an authentication or release URL without involving a command shell.
///
/// External URLs are deliberately limited to HTTPS and may not contain URL
/// credentials. This keeps provider-controlled values away from shell parsing
/// and prevents accidental downgrade to an insecure authentication endpoint.
pub fn open_https_url(value: &str) -> Result<(), Box<dyn std::error::Error>> {
    let url = validated_https_url(value)?;
    open_validated_url(url.as_str())
}

fn validated_https_url(value: &str) -> Result<url::Url, Box<dyn std::error::Error>> {
    if value.len() > MAX_EXTERNAL_URL_LENGTH {
        return Err("external URL is too long".into());
    }
    let parsed = url::Url::parse(value).map_err(|_| "external URL is invalid")?;
    if parsed.scheme() != "https" {
        return Err("only HTTPS external URLs are allowed".into());
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err("external URLs may not contain credentials".into());
    }
    if parsed.host_str().is_none() {
        return Err("external URL must include a host".into());
    }
    Ok(parsed)
}

#[cfg(unix)]
fn open_validated_url(value: &str) -> Result<(), Box<dyn std::error::Error>> {
    let status = Command::new("xdg-open")
        .arg(value)
        .status()
        .map_err(|error| format!("could not start xdg-open: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("xdg-open failed with {status}").into())
    }
}

#[cfg(windows)]
fn open_validated_url(value: &str) -> Result<(), Box<dyn std::error::Error>> {
    use std::os::windows::ffi::OsStrExt;
    use windows::{
        Win32::UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL},
        core::{PCWSTR, w},
    };

    let wide = std::ffi::OsStr::new(value)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let result = unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            PCWSTR(wide.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    if result.0 as isize > 32 {
        Ok(())
    } else {
        Err(format!(
            "Windows could not open the URL (ShellExecuteW={})",
            result.0 as isize
        )
        .into())
    }
}

#[cfg(not(any(unix, windows)))]
fn open_validated_url(_value: &str) -> Result<(), Box<dyn std::error::Error>> {
    Err("opening external URLs is not supported on this platform".into())
}

/// Resolve the desktop executable next to the CLI first. Release installers
/// place all components together, while PATH remains a development fallback.
pub fn desktop_command() -> Command {
    let name = if cfg!(windows) {
        "llm-meter-desktop.exe"
    } else {
        "llm-meter-desktop"
    };
    let sibling = std::env::current_exe().ok().and_then(|current| {
        let candidate = current.parent()?.join(name);
        candidate.is_file().then_some(candidate)
    });
    Command::new(sibling.unwrap_or_else(|| PathBuf::from(name)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_https_without_url_credentials() {
        let parsed = validated_https_url("https://example.com/oauth?state=abc").unwrap();
        assert_eq!(parsed.host_str(), Some("example.com"));
    }

    #[test]
    fn rejects_non_https_and_url_credentials() {
        assert!(validated_https_url("http://example.com/oauth").is_err());
        assert!(validated_https_url("file:///tmp/token").is_err());
        assert!(validated_https_url("https://user:secret@example.com/").is_err());
    }

    #[test]
    fn rejects_oversized_external_url() {
        let value = format!(
            "https://example.com/{}",
            "a".repeat(MAX_EXTERNAL_URL_LENGTH)
        );
        assert!(validated_https_url(&value).is_err());
    }
}
